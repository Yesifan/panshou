use std::time::{Duration, Instant};

use num_bigint::BigUint;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct Challenge {
    #[serde(default)]
    pub id: String,
    #[serde(default, rename = "N")]
    pub modulus: String,
    #[serde(default, rename = "x")]
    pub initial: String,
    #[serde(default)]
    pub t: usize,
    #[serde(default)]
    pub salt: String,
    #[serde(default)]
    pub diff: usize,
    #[serde(default)]
    pub challenge: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Solution {
    Pow { id: Option<String>, y: String },
    Legacy { id: String, nonces: Vec<usize> },
}

#[derive(Debug, Error)]
pub enum ChallengeError {
    #[error("invalid PoW challenge field {0}")]
    InvalidPow(&'static str),
    #[error("legacy challenge is incomplete")]
    InvalidLegacy,
    #[error("legacy challenge target could not be solved")]
    Unsolved,
}

pub fn solve_pow(challenge: &Challenge) -> Result<String, ChallengeError> {
    let modulus = BigUint::parse_bytes(challenge.modulus.as_bytes(), 16)
        .filter(|v| v != &BigUint::from(0u8))
        .ok_or(ChallengeError::InvalidPow("N"))?;
    let mut value = BigUint::parse_bytes(challenge.initial.as_bytes(), 16)
        .ok_or(ChallengeError::InvalidPow("x"))?;
    if challenge.t == 0 {
        return Err(ChallengeError::InvalidPow("t"));
    }
    for _ in 0..challenge.t {
        value = (&value * &value) % &modulus;
    }
    Ok(value.to_str_radix(16))
}

pub async fn solve_inline_pow(
    challenge: &Challenge,
    minimum: Duration,
) -> Result<Solution, ChallengeError> {
    if challenge.id.is_empty() {
        return Err(ChallengeError::InvalidPow("id"));
    }
    let started = Instant::now();
    let y = solve_pow(challenge)?;
    if let Some(wait) = minimum.checked_sub(started.elapsed()) {
        tokio::time::sleep(wait).await
    }
    Ok(Solution::Pow {
        id: Some(challenge.id.clone()),
        y,
    })
}

pub fn solve_remote_pow(challenge: &Challenge) -> Result<Solution, ChallengeError> {
    Ok(Solution::Pow {
        id: None,
        y: solve_pow(challenge)?,
    })
}

pub fn solve_legacy(challenge: &Challenge) -> Result<Solution, ChallengeError> {
    if challenge.id.is_empty()
        || challenge.salt.is_empty()
        || challenge.diff == 0
        || challenge.challenge.is_empty()
    {
        return Err(ChallengeError::InvalidLegacy);
    }
    let targets = challenge
        .challenge
        .iter()
        .map(|v| v.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut nonces = vec![None; targets.len()];
    for nonce in 0..=challenge.diff {
        let hash = format!(
            "{:x}",
            Sha256::digest(format!("{nonce}{}", challenge.salt).as_bytes())
        );
        for (index, target) in targets.iter().enumerate() {
            if nonces[index].is_none() && hash == *target {
                nonces[index] = Some(nonce)
            }
        }
        if nonces.iter().all(Option::is_some) {
            break;
        }
    }
    let nonces = nonces
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or(ChallengeError::Unsolved)?;
    Ok(Solution::Legacy {
        id: challenge.id.clone(),
        nonces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pow_vector() {
        let c = Challenge {
            id: "i".into(),
            modulus: "11".into(),
            initial: "2".into(),
            t: 3,
            salt: String::new(),
            diff: 0,
            challenge: vec![],
        };
        assert_eq!(solve_pow(&c).unwrap(), "1");
    }
    #[test]
    fn legacy_vector() {
        let target = format!("{:x}", Sha256::digest(b"2salt"));
        let c = Challenge {
            id: "i".into(),
            modulus: String::new(),
            initial: String::new(),
            t: 0,
            salt: "salt".into(),
            diff: 5,
            challenge: vec![target],
        };
        assert_eq!(
            solve_legacy(&c).unwrap(),
            Solution::Legacy {
                id: "i".into(),
                nonces: vec![2]
            }
        );
    }
    #[tokio::test]
    async fn inline_solution_carries_id() {
        let challenge = Challenge {
            id: "inline".into(),
            modulus: "11".into(),
            initial: "2".into(),
            t: 1,
            salt: String::new(),
            diff: 0,
            challenge: vec![],
        };
        assert_eq!(
            solve_inline_pow(&challenge, Duration::ZERO).await.unwrap(),
            Solution::Pow {
                id: Some("inline".into()),
                y: "4".into()
            }
        );
    }

    #[test]
    fn rejects_incomplete_challenges() {
        let challenge: Challenge = serde_json::from_str("{}").unwrap();
        assert!(solve_pow(&challenge).is_err());
        assert!(solve_legacy(&challenge).is_err());
    }
}
