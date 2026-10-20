// Included in streaming::tests to exercise the actual CLI final-output contract.
struct NewCloudProvider;

#[async_trait]
impl Provider for NewCloudProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::stateless("new-cloud-fixture", 3, KeywordFilterMode::Provider)
    }

    async fn search(&self, _: &SearchContext, _: &str) -> Result<Vec<SearchResult>, ProviderError> {
        let mut result = SearchResult::provider("new-cloud-fixture", "new-cloud-fixture");
        result.title = "query".into();
        result.links.extend([
            Link::new("https://share.weiyun.com/demo"),
            Link::new("https://demo.lanzoui.com/demo"),
        ]);
        Ok(vec![result])
    }
}

#[tokio::test]
async fn new_cloud_filters_require_no_check_to_keep_unsupported_links() {
    use crate::cli::{Cli, Command};
    use clap::Parser;
    for cloud in ["weiyun", "lanzou"] {
        for no_check in [false, true] {
            let mut argv = vec!["pansou", "search", "query", "--cloud", cloud];
            if no_check {
                argv.push("--no-check");
            }
            let cli = Cli::try_parse_from(argv).unwrap();
            let Command::Search(args) = cli.command else {
                panic!("expected search")
            };
            let (engine, mut options) = setup(vec![Arc::new(NewCloudProvider)]);
            options.clouds = args.clouds;
            let checker =
                (!args.no_check).then(|| Arc::new(CheckEngine::new(reqwest::Client::new(), None)));
            let mut output = Vec::new();
            let code = drive(
                engine,
                options,
                checker,
                CheckOptions::default(),
                OutputFormat::Json,
                !args.no_check,
                false,
                true,
                None,
                &mut output,
                futures::future::pending(),
            )
            .await
            .unwrap();
            assert_eq!(code, EXIT_OK);
            let document: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(document["total_links"], usize::from(no_check));
            if !no_check {
                assert_eq!(document["checks"]["candidates"], 1);
                assert_eq!(document["checks"]["checked"], 1);
                assert_eq!(document["checks"]["valid"], 0);
            } else {
                assert!(String::from_utf8(output).unwrap().contains(cloud));
            }
        }
    }
}
