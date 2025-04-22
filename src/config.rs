use crate::qbittorrent;
use serde::de::Unexpected;
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::str::FromStr;

#[derive(Debug)]
pub enum ConfigError {
    Deserialization(serde_yaml::Error),
    Io(io::Error),
}

#[derive(Deserialize, PartialEq, Debug)]
pub struct Config {
    pub server: ServerConfig,
    pub rules: RuleList,
}

#[derive(Debug, PartialEq)]
struct Comparison<T> {
    operator: ComparisonOperator,
    value: T,
}

impl<T: PartialOrd> Comparison<T> {
    fn compare(&self, value: T) -> bool {
        match self.operator {
            ComparisonOperator::GreaterThan => value > self.value,
            ComparisonOperator::GreaterThanOrEqual => value >= self.value,
            ComparisonOperator::LessThan => value < self.value,
            ComparisonOperator::LessThanOrEqual => value <= self.value,
        }
    }
}

#[derive(Debug, PartialEq)]
enum ComparisonOperator {
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
}

impl fmt::Display for ComparisonOperator {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let symbol = match self {
            ComparisonOperator::GreaterThan => ">",
            ComparisonOperator::GreaterThanOrEqual => ">=",
            ComparisonOperator::LessThan => "<",
            ComparisonOperator::LessThanOrEqual => "<=",
        };
        write!(f, "{}", symbol)
    }
}

impl<'de, T: FromStr> serde::Deserialize<'de> for Comparison<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(d)?;
        let Some(pos) = s.find(|c| c != '>' && c != '<' && c != '=') else {
            return Err(Error::invalid_value(
                Unexpected::Str(&s),
                &"a number prefixed with '>', '>=', '<' or '<='",
            ));
        };
        let (prefix, value) = s.split_at(pos);
        let operator = match prefix {
            "<" => ComparisonOperator::LessThan,
            "<=" => ComparisonOperator::LessThanOrEqual,
            ">" => ComparisonOperator::GreaterThan,
            ">=" => ComparisonOperator::GreaterThanOrEqual,
            prefix => {
                return Err(Error::invalid_value(
                    Unexpected::Other(&format!("prefix \"{}\"", &prefix)),
                    &"prefix '>', '>=', '<' or '<='",
                ))
            }
        };
        let value = match value.parse::<T>() {
            Ok(value) => value,
            Err(_) => {
                return Err(Error::invalid_value(
                    Unexpected::Str(value),
                    &"a suitable number",
                ));
            }
        };
        Ok(Self { operator, value })
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let yaml = Self::load_file(path).map_err(ConfigError::Io)?;
        let config: Self = serde_yaml::from_str(&yaml).map_err(ConfigError::Deserialization)?;
        Ok(config)
    }

    fn load_file(path: &Path) -> Result<String, io::Error> {
        let mut file = File::open(path)?;
        let mut file_content = String::new();
        file.read_to_string(&mut file_content)?;
        Ok(file_content)
    }
}

#[derive(Deserialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    category: Option<String>,
    seeding_time: Option<Comparison<usize>>,
    tags: Option<qbittorrent::TagList>,
    pub limits: RuleLimits,
}

impl Rule {
    fn matches(&self, torrent: &qbittorrent::Torrent) -> bool {
        if let Some(category) = &self.category {
            if category != &torrent.category {
                return false;
            }
        }
        if let Some(seeding_time) = &self.seeding_time {
            if !seeding_time.compare(torrent.seeding_time / 60) {
                return false;
            }
        }
        if let Some(tags) = &self.tags {
            if tags != &torrent.tags {
                return false;
            }
        }
        true
    }

    pub fn needs_update(&self, torrent: &qbittorrent::Torrent) -> bool {
        if let Some(ratio) = &self.limits.ratio {
            if &torrent.max_ratio != ratio {
                log::debug!("Torrent {} has incorrect ratio", torrent.name);
                return true;
            }
        }
        if let Some(minutes) = &self.limits.minutes {
            if &torrent.max_seeding_time != minutes {
                log::debug!("Torrent {} has incorrect max seeding time", torrent.name);
                return true;
            }
        }
        false
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut conditions = Vec::<String>::new();
        if let Some(category) = &self.category {
            conditions.push(format!("category = {}", category));
        }
        if let Some(seeding_time) = &self.seeding_time {
            conditions.push(format!(
                "seeding time {} {} minutes",
                seeding_time.operator, seeding_time.value
            ));
        }
        if let Some(tags) = &self.tags {
            conditions.push(format!("tags = {}", tags));
        }
        let ratio = match self.limits.ratio {
            Some(ratio) => Cow::from(ratio.to_string()),
            None => Cow::from(crate::UNLIMITED),
        };
        let minutes = match self.limits.minutes {
            Some(minutes) => Cow::from(minutes.to_string()),
            None => Cow::from(crate::UNLIMITED),
        };
        write!(f, "{} => {} ratio and {} minutes", conditions.join(", "), ratio, minutes)
    }
}

#[derive(Deserialize, PartialEq, Debug)]
pub struct RuleLimits {
    pub ratio: Option<qbittorrent::Ratio>,
    pub minutes: Option<qbittorrent::MaxSeedingTime>,
}

#[derive(Deserialize, PartialEq, Debug)]
pub struct RuleList(Vec<Rule>);

impl RuleList {
    pub fn find(&self, torrent: &qbittorrent::Torrent) -> Option<&Rule> {
        self.0.iter().find(|&rule| rule.matches(torrent))
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Rule> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

#[derive(Deserialize, PartialEq, Debug, Default)]
pub struct ServerConfig {
    pub address: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    mod comparison {
        use super::*;
        use test_case::test_case;

        #[test_case(111, false ; "less")]
        #[test_case(222, false ; "equal")]
        #[test_case(333, true ; "greater")]
        fn test_compare_gt(value: usize, expected: bool) {
            let comparison = Comparison::<usize> {
                operator: ComparisonOperator::GreaterThan,
                value: 222,
            };
            assert_eq!(comparison.compare(value), expected);
        }

        #[test_case(222, false ; "less")]
        #[test_case(333, true ; "equal")]
        #[test_case(444, true ; "greater")]
        fn test_compare_gte(value: usize, expected: bool) {
            let comparison = Comparison::<usize> {
                operator: ComparisonOperator::GreaterThanOrEqual,
                value: 333,
            };
            assert_eq!(comparison.compare(value), expected);
        }

        #[test_case(333, true ; "less")]
        #[test_case(444, false ; "equal")]
        #[test_case(555, false ; "more")]
        fn test_compare_lt(value: usize, expected: bool) {
            let comparison = Comparison::<usize> {
                operator: ComparisonOperator::LessThan,
                value: 444,
            };
            assert_eq!(comparison.compare(value), expected);
        }

        #[test_case(444, true ; "less")]
        #[test_case(555, true ; "equal")]
        #[test_case(666, false ; "more")]
        fn test_compare_lte(value: usize, expected: bool) {
            let comparison = Comparison::<usize> {
                operator: ComparisonOperator::LessThanOrEqual,
                value: 555,
            };
            assert_eq!(comparison.compare(value), expected);
        }

        mod deserialize {
            use super::*;
            use serde_test::{assert_de_tokens, assert_de_tokens_error, Token};

            #[test]
            fn test_error_nan() {
                assert_de_tokens_error::<Comparison<usize>>(
                    &[Token::Str(">abc")],
                    "invalid value: string \"abc\", expected a suitable number",
                );
            }

            #[test]
            fn test_error_no_value() {
                assert_de_tokens_error::<Comparison<usize>>(
                    &[Token::Str("<=")],
                    "invalid value: string \"<=\", \
                    expected a number prefixed with '>', '>=', '<' or '<='",
                );
            }

            #[test]
            fn test_error_signed_usize() {
                assert_de_tokens_error::<Comparison<usize>>(
                    &[Token::Str(">=-1234")],
                    "invalid value: string \"-1234\", expected a suitable number",
                );
            }

            #[test]
            fn test_error_unknown_prefix() {
                assert_de_tokens_error::<Comparison<usize>>(
                    &[Token::Str("=100")],
                    "invalid value: prefix \"=\", expected prefix '>', '>=', '<' or '<='",
                );
            }

            #[test]
            fn test_gt() {
                let comparison = Comparison::<i32> {
                    operator: ComparisonOperator::GreaterThan,
                    value: 963,
                };
                assert_de_tokens(&comparison, &[Token::Str(">963")]);
            }

            #[test]
            fn test_gte() {
                let comparison = Comparison::<u32> {
                    operator: ComparisonOperator::GreaterThanOrEqual,
                    value: 1234,
                };
                assert_de_tokens(&comparison, &[Token::Str(">=1234")]);
            }

            #[test]
            fn test_lt() {
                let comparison = Comparison::<usize> {
                    operator: ComparisonOperator::LessThan,
                    value: 3,
                };
                assert_de_tokens(&comparison, &[Token::Str("<3")]);
            }

            #[test]
            fn test_lte() {
                let comparison = Comparison::<i64> {
                    operator: ComparisonOperator::LessThanOrEqual,
                    value: -50000,
                };
                assert_de_tokens(&comparison, &[Token::Str("<=-50000")]);
            }
        }
    }
}
