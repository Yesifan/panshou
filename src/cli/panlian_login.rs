//! A single cancellation scope owns the entire manual Panlian dialogue.
//! Input is readiness-polled on the command task, never delegated to a blocking worker.
use super::{
    Config, PanlianEndpoints, PanlianLoginArgs, PanlianLoginOutcome, PanlianProvider, StateStore,
    required_username, usage,
};
use crate::{
    core::CloudType,
    http::{ClientOptions, HttpClientFactory, Session},
};
use async_trait::async_trait;
use std::{
    future::Future,
    io::{self, IsTerminal, Write},
    path::Path,
    str::FromStr,
    time::Duration,
};

pub(super) async fn run(
    args: PanlianLoginArgs,
    store: StateStore,
    config: &Config,
    options: ClientOptions,
) -> anyhow::Result<i32> {
    let _ = store.profile_path("panlian", &args.profile)?;
    if args.remember_credentials {
        return Err(usage(
            "Panlian no longer saves passwords or automatically renews expired sessions; omit --remember-credentials",
        ));
    }
    let terminal = io::stdin().is_terminal();
    if !terminal && !args.interactive {
        return Err(usage(
            "Panlian requires manual captcha interaction; use a terminal or explicitly pass --interactive",
        ));
    }
    let _ = required_username(args.username.as_deref())?;
    let blocked = config
        .providers
        .panlian
        .blocked_clouds
        .iter()
        .map(|value| CloudType::from_str(value))
        .collect::<Result<Vec<_>, _>>()?;
    let provider = PanlianProvider::load(store, PanlianEndpoints::default(), blocked)?;
    let session = HttpClientFactory::new().session(&options)?;
    let mut input = ConsoleInput;
    let code = run_cancellable(
        &args,
        &provider,
        &session,
        &mut input,
        terminal,
        |path| {
            eprintln!("Open the graphic captcha: {}", path.display());
            if terminal {
                super::open_with_system_viewer(path);
            }
        },
        tokio::signal::ctrl_c(),
    )
    .await?;
    if code == 0 {
        println!("panlian/{}: logged in and ready", args.profile);
        println!(
            "Verify with: pansou provider status panlian --profile {}",
            args.profile
        );
        println!(
            "Optional global filter: pansou provider configure panlian --blocked-cloud <CLOUD>"
        );
    }
    Ok(code)
}

#[async_trait]
trait DialogueInput: Send {
    /// `tty` requests the controlling terminal; false consumes exactly one stdin line.
    /// Every field is hidden on a TTY, including captcha codes and bound email.
    async fn line(&mut self, prompt: &str, tty: bool) -> anyhow::Result<Option<String>>;
}

#[allow(clippy::too_many_arguments)]
async fn run_cancellable<I, S, C>(
    args: &PanlianLoginArgs,
    provider: &PanlianProvider,
    session: &Session,
    input: &mut I,
    terminal: bool,
    mut show_image: S,
    cancellation: C,
) -> anyhow::Result<i32>
where
    I: DialogueInput,
    S: FnMut(&Path),
    C: Future<Output = io::Result<()>>,
{
    // Dropping dialogue drops raw-mode guards, the image handle and owned HTTP
    // futures together. No request/input task can continue after this returns.
    let dialogue = async {
        let username = required_username(args.username.as_deref())?;
        let Some(password) = input
            .line("Password: ", terminal || !args.password_stdin)
            .await?
        else {
            return Ok(130);
        };
        let challenge = provider.captcha(session).await?;
        let image = challenge.write_private_image()?;
        show_image(image.path());
        let Some(code) = input
            .line("Graphic captcha (empty cancels): ", terminal)
            .await?
        else {
            return Ok(130);
        };
        let outcome = provider
            .login_with_captcha(
                session,
                &args.profile,
                username,
                &password,
                &challenge.id,
                &code,
            )
            .await?;
        if let PanlianLoginOutcome::EmailRequired(pending) = outcome {
            let Some(email) = input
                .line(
                    "Account's bound email (not an OTP; empty cancels): ",
                    terminal,
                )
                .await?
            else {
                return Ok(130);
            };
            if email.trim().is_empty() {
                return Ok(130);
            }
            provider
                .confirm_email(session, &args.profile, &pending, &email)
                .await?;
        }
        drop(image);
        Ok::<i32, anyhow::Error>(0)
    };
    tokio::select! {
        biased;
        signal = cancellation => { signal?; Ok(130) }
        result = dialogue => result,
    }
}

struct ConsoleInput;
struct RawMode;
impl RawMode {
    fn enter() -> io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}
impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}
#[async_trait]
impl DialogueInput for ConsoleInput {
    async fn line(&mut self, prompt: &str, tty: bool) -> anyhow::Result<Option<String>> {
        eprint!("{prompt}");
        io::stderr().flush()?;
        if tty {
            terminal_line().await
        } else {
            stdin_line().await
        }
    }
}
async fn terminal_line() -> anyhow::Result<Option<String>> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    let guard = RawMode::enter()?;
    let mut value = String::new();
    let result = loop {
        if event::poll(Duration::ZERO)? {
            // poll/read have one consumer, so read cannot wait for another event.
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                match key.code {
                    KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        break None;
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        value.clear()
                    }
                    KeyCode::Enter => break (!value.is_empty()).then_some(value),
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Char(ch)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        value.push(ch)
                    }
                    _ => {}
                }
                if value.len() > 16 * 1024 {
                    return Err(usage("login input exceeds 16 KiB"));
                }
            }
            tokio::task::yield_now().await;
        } else {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    drop(guard);
    eprintln!();
    Ok(result)
}

enum StdinByte {
    Pending,
    End,
    Byte(u8),
}
async fn stdin_line() -> anyhow::Result<Option<String>> {
    stdin_line_from(stdin_byte).await
}
async fn stdin_line_from(
    mut next: impl FnMut() -> io::Result<StdinByte>,
) -> anyhow::Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        match next()? {
            StdinByte::Pending => tokio::time::sleep(Duration::from_millis(20)).await,
            StdinByte::End | StdinByte::Byte(b'\n') => {
                while bytes.last() == Some(&b'\r') {
                    bytes.pop();
                }
                if bytes.is_empty() {
                    return Ok(None);
                }
                // Do not include the rejected input in diagnostics.
                return String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| usage("login input must be UTF-8"));
            }
            StdinByte::Byte(byte) => {
                bytes.push(byte);
                if bytes.len() > 16 * 1024 {
                    return Err(usage("login input exceeds 16 KiB"));
                }
            }
        }
    }
}
#[cfg(unix)]
fn stdin_byte() -> io::Result<StdinByte> {
    use rustix::event::{PollFd, PollFlags, Timespec};
    let stdin = io::stdin();
    let mut descriptors = [PollFd::new(&stdin, PollFlags::IN)];
    let ready = rustix::event::poll(
        &mut descriptors,
        Some(&Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        }),
    );
    match ready {
        Err(rustix::io::Errno::INTR) => return Ok(StdinByte::Pending),
        Err(error) => return Err(error.into()),
        Ok(0) => return Ok(StdinByte::Pending),
        Ok(_) => {}
    }
    if descriptors[0]
        .revents()
        .intersects(PollFlags::ERR | PollFlags::NVAL)
    {
        return Err(io::Error::other("cannot poll login stdin"));
    }
    let mut byte = [0];
    match rustix::io::read(&stdin, &mut byte) {
        Ok(0) => Ok(StdinByte::End),
        Ok(_) => Ok(StdinByte::Byte(byte[0])),
        Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => Ok(StdinByte::Pending),
        Err(error) => Err(error.into()),
    }
}
#[cfg(windows)]
fn stdin_byte() -> io::Result<StdinByte> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::{ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF},
        Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType, ReadFile},
        System::Pipes::PeekNamedPipe,
    };
    let stdin = io::stdin();
    let handle = stdin.as_raw_handle();
    // SAFETY: stdin owns this live handle; the stack buffers outlive each
    // synchronous call. Pipe reads occur only after readiness is established.
    unsafe {
        match GetFileType(handle) {
            FILE_TYPE_PIPE => {
                let mut available = 0;
                if PeekNamedPipe(
                    handle,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                ) == 0
                {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                        return Ok(StdinByte::End);
                    }
                    return Err(error);
                }
                if available == 0 {
                    return Ok(StdinByte::Pending);
                }
            }
            FILE_TYPE_DISK => {}
            _ => {
                return Err(io::Error::other(
                    "login stdin must be a terminal, file, or pipe",
                ));
            }
        }
        let mut byte = [0u8];
        let mut read = 0;
        if ReadFile(
            handle,
            byte.as_mut_ptr(),
            1,
            &mut read,
            std::ptr::null_mut(),
        ) == 0
        {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(),Some(code) if code==ERROR_BROKEN_PIPE as i32 || code==ERROR_HANDLE_EOF as i32)
            {
                return Ok(StdinByte::End);
            }
            return Err(error);
        }
        if read == 0 {
            Ok(StdinByte::End)
        } else {
            Ok(StdinByte::Byte(byte[0]))
        }
    }
}
#[cfg(not(any(unix, windows)))]
fn stdin_byte() -> io::Result<StdinByte> {
    Err(io::Error::other(
        "non-TTY login input is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::panlian::PanlianProfile;
    use serde_json::json;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };
    use tokio::sync::Notify;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string_contains, method, path},
    };

    struct Input {
        lines: VecDeque<String>,
        waiting: Arc<Notify>,
        modes: Vec<bool>,
    }
    #[async_trait]
    impl DialogueInput for Input {
        async fn line(&mut self, _: &str, tty: bool) -> anyhow::Result<Option<String>> {
            self.modes.push(tty);
            if let Some(line) = self.lines.pop_front() {
                return Ok(Some(line));
            }
            self.waiting.notify_one();
            std::future::pending().await
        }
    }
    fn args() -> PanlianLoginArgs {
        PanlianLoginArgs {
            interactive: true,
            profile: "main".into(),
            username: Some("fixture".into()),
            password_stdin: true,
            remember_credentials: false,
        }
    }
    fn captcha() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":{"id":"private-captcha","image":"data:image/png;base64,aW1hZ2U="}}))
    }
    fn email_required() -> ResponseTemplate {
        ResponseTemplate::new(400).set_body_json(json!({"success":false,"details":{"email_verify_required":true,"confirm_id":"private-confirm","email":"f***@example.test"}}))
    }
    fn active() -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("set-cookie", "admin_session=fixture; Path=/; HttpOnly")
            .set_body_json(json!({"success":true}))
    }
    #[tokio::test]
    async fn cancellation_at_every_dialogue_stage_cleans_up_without_activation() {
        for stage in [
            "password",
            "captcha-request",
            "captcha-input",
            "login-request",
            "email-input",
            "confirm-request",
        ] {
            let server = MockServer::start().await;
            let directory = tempfile::tempdir().unwrap();
            let store = StateStore::with_cipher(directory.path(), None);
            let provider = PanlianProvider::load(
                store.clone(),
                PanlianEndpoints {
                    base: url::Url::parse(&format!("{}/", server.uri())).unwrap(),
                },
                [],
            )
            .unwrap();
            let session = HttpClientFactory::new()
                .session(&ClientOptions::default())
                .unwrap();
            let waiting = Arc::new(Notify::new());
            let line_count = match stage {
                "password" => 0,
                "captcha-request" | "captcha-input" => 1,
                "login-request" | "email-input" => 2,
                _ => 3,
            };
            let mut input = Input {
                lines: ["password", "1234", "fixture@example.test"]
                    .into_iter()
                    .take(line_count)
                    .map(str::to_owned)
                    .collect(),
                waiting: waiting.clone(),
                modes: vec![],
            };
            let captcha_count = u64::from(stage != "password");
            let notice = waiting.clone();
            let delayed = stage == "captcha-request";
            Mock::given(method("GET"))
                .and(path("/api/auth/captcha"))
                .respond_with(move |_: &wiremock::Request| {
                    if delayed {
                        notice.notify_one();
                        captcha().set_delay(Duration::from_secs(60))
                    } else {
                        captcha()
                    }
                })
                .expect(captcha_count)
                .mount(&server)
                .await;
            let login_count = u64::from(matches!(
                stage,
                "login-request" | "email-input" | "confirm-request"
            ));
            let notice = waiting.clone();
            let delayed = stage == "login-request";
            Mock::given(method("POST"))
                .and(path("/api/auth/login"))
                .and(body_string_contains("password=password"))
                .and(body_string_contains("captcha_code=1234"))
                .respond_with(move |_: &wiremock::Request| {
                    if delayed {
                        notice.notify_one();
                        email_required().set_delay(Duration::from_secs(60))
                    } else {
                        email_required()
                    }
                })
                .expect(login_count)
                .mount(&server)
                .await;
            let notice = waiting.clone();
            Mock::given(method("POST"))
                .and(path("/api/auth/login/confirm-email"))
                .respond_with(move |_: &wiremock::Request| {
                    notice.notify_one();
                    active().set_delay(Duration::from_secs(60))
                })
                .expect(u64::from(stage == "confirm-request"))
                .mount(&server)
                .await;
            let images = Arc::new(Mutex::new(Vec::new()));
            let seen = images.clone();
            let cancel = async {
                waiting.notified().await;
                Ok(())
            };
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                run_cancellable(
                    &args(),
                    &provider,
                    &session,
                    &mut input,
                    false,
                    move |path| {
                        assert!(path.exists());
                        seen.lock().unwrap().push(path.to_owned());
                    },
                    cancel,
                ),
            )
            .await
            .expect(stage)
            .unwrap();
            assert_eq!(result, 130, "{stage}");
            assert!(provider.profile_names().unwrap().is_empty(), "{stage}");
            assert!(
                store
                    .load::<PanlianProfile>("panlian", "main")
                    .unwrap()
                    .is_none(),
                "{stage}"
            );
            let paths = images.lock().unwrap();
            assert_eq!(
                paths.len(),
                usize::from(!matches!(stage, "password" | "captcha-request")),
                "{stage}"
            );
            assert!(
                paths.iter().all(|path| !path.exists()),
                "{stage}: captcha file survived cancellation"
            );
        }
    }
    #[tokio::test]
    async fn stdin_reader_does_not_consume_later_dialogue_lines() {
        let mut bytes = b"password\r\n1234\nfixture@example.test\n"
            .iter()
            .copied()
            .collect::<VecDeque<_>>();
        for expected in ["password", "1234", "fixture@example.test"] {
            let value =
                stdin_line_from(|| Ok(bytes.pop_front().map_or(StdinByte::End, StdinByte::Byte)))
                    .await
                    .unwrap();
            assert_eq!(value.as_deref(), Some(expected));
        }
        assert!(bytes.is_empty());
        assert!(
            tokio::time::timeout(
                Duration::from_millis(1),
                stdin_line_from(|| Ok(StdinByte::Pending))
            )
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn password_stdin_consumes_one_line_and_confirmation_activates_only_at_the_end() {
        let server = MockServer::start().await;
        let directory = tempfile::tempdir().unwrap();
        let store = StateStore::with_cipher(directory.path(), None);
        let provider = PanlianProvider::load(
            store.clone(),
            PanlianEndpoints {
                base: url::Url::parse(&format!("{}/", server.uri())).unwrap(),
            },
            [],
        )
        .unwrap();
        let session = HttpClientFactory::new()
            .session(&ClientOptions::default())
            .unwrap();
        Mock::given(method("GET"))
            .and(path("/api/auth/captcha"))
            .respond_with(captcha())
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/auth/login"))
            .and(body_string_contains("password=password"))
            .and(body_string_contains("captcha_code=1234"))
            .respond_with(email_required())
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/auth/login/confirm-email"))
            .and(body_string_contains("email=fixture%40example.test"))
            .respond_with(active())
            .expect(1)
            .mount(&server)
            .await;
        let mut input = Input {
            lines: ["password", "1234", "fixture@example.test"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            waiting: Arc::new(Notify::new()),
            modes: vec![],
        };
        let images = Arc::new(Mutex::new(Vec::new()));
        let seen = images.clone();
        let code = run_cancellable(
            &args(),
            &provider,
            &session,
            &mut input,
            false,
            move |path| seen.lock().unwrap().push(path.to_owned()),
            std::future::pending::<io::Result<()>>(),
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(input.modes, [false, false, false]);
        assert!(
            store
                .load::<PanlianProfile>("panlian", "main")
                .unwrap()
                .unwrap()
                .password
                .is_none()
        );
        assert!(images.lock().unwrap().iter().all(|path| !path.exists()));
    }
}
