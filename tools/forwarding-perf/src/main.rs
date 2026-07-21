//! Entry point for the manually invoked forwarding performance harness.

mod database;
mod load_client;
mod metrics;
mod mock_upstream;
mod orchestrator;
mod process;
mod report;
mod scenario;

use std::{
    error::Error,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use load_client::LoadOptions;
use orchestrator::RunOptions;
use scenario::ApiKind;

#[tokio::main]
async fn main() {
    if let Err(error) = dispatch(std::env::args().skip(1).collect()).await {
        eprintln!("ai-gateway-perf: {error}");
        std::process::exit(1);
    }
}

async fn dispatch(arguments: Vec<String>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let Some(command) = arguments.first().map(String::as_str) else {
        print_usage();
        return Err("missing command".into());
    };
    match command {
        "run" => orchestrator::run(parse_run_options(&arguments[1..])?).await,
        "mock-upstream" => {
            let address = parse_mock_options(&arguments[1..])?;
            mock_upstream::run(address).await
        }
        "load-client" => {
            let (options, output) = parse_load_options(&arguments[1..])?;
            let result = load_client::run(options).await?;
            tokio::fs::write(&output, serde_json::to_vec_pretty(&result)?).await?;
            println!(
                "{}: {:.1} successful requests/s, p99 {:.3} ms, error rate {:.4}%",
                result.scenario,
                result.success_rps,
                result.latency.p99_us as f64 / 1_000.0,
                result.error_rate * 100.0
            );
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        _ => {
            print_usage();
            Err(format!("unknown command {command:?}").into())
        }
    }
}

fn parse_run_options(arguments: &[String]) -> Result<RunOptions, Box<dyn Error + Send + Sync>> {
    let repo_root = repo_root();
    let mut options = RunOptions::defaults(&repo_root);
    let mut index = 0;
    while index < arguments.len() {
        let flag = &arguments[index];
        index += 1;
        match flag.as_str() {
            "--keep-database" => options.keep_database = true,
            "--profile" => {
                options.profile = required_value(arguments, &mut index, flag)?.parse()?;
            }
            "--database-admin-url" => {
                options.database_admin_url = required_value(arguments, &mut index, flag)?.into();
            }
            "--gateway-bin" => {
                options.gateway_bin = PathBuf::from(required_value(arguments, &mut index, flag)?);
            }
            "--report-dir" => {
                options.report_root = PathBuf::from(required_value(arguments, &mut index, flag)?);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown run option {flag:?}").into()),
        }
    }
    Ok(options)
}

fn parse_mock_options(arguments: &[String]) -> Result<SocketAddr, Box<dyn Error + Send + Sync>> {
    let mut listen = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = &arguments[index];
        index += 1;
        match flag.as_str() {
            "--listen" => {
                listen = Some(required_value(arguments, &mut index, flag)?.parse::<SocketAddr>()?);
            }
            _ => return Err(format!("unknown mock-upstream option {flag:?}").into()),
        }
    }
    listen.ok_or_else(|| "mock-upstream requires --listen".into())
}

fn parse_load_options(
    arguments: &[String],
) -> Result<(LoadOptions, PathBuf), Box<dyn Error + Send + Sync>> {
    let mut scenario = None;
    let mut target = None;
    let mut api_kind = None;
    let mut streamed = false;
    let mut concurrency = None;
    let mut warmup_seconds = None;
    let mut duration_seconds = None;
    let mut timeout_seconds = None;
    let mut api_key = None;
    let mut model = None;
    let mut output = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = &arguments[index];
        index += 1;
        match flag.as_str() {
            "--scenario" => scenario = Some(required_value(arguments, &mut index, flag)?.into()),
            "--target" => target = Some(required_value(arguments, &mut index, flag)?.into()),
            "--api-format" => {
                api_kind = Some(ApiKind::from_str(required_value(
                    arguments, &mut index, flag,
                )?)?)
            }
            "--stream" => streamed = true,
            "--concurrency" => {
                concurrency = Some(required_value(arguments, &mut index, flag)?.parse()?)
            }
            "--warmup-seconds" => {
                warmup_seconds = Some(required_value(arguments, &mut index, flag)?.parse()?)
            }
            "--duration-seconds" => {
                duration_seconds = Some(required_value(arguments, &mut index, flag)?.parse()?)
            }
            "--timeout-seconds" => {
                timeout_seconds = Some(required_value(arguments, &mut index, flag)?.parse()?)
            }
            "--api-key" => api_key = Some(required_value(arguments, &mut index, flag)?.into()),
            "--model" => model = Some(required_value(arguments, &mut index, flag)?.into()),
            "--output" => {
                output = Some(PathBuf::from(required_value(arguments, &mut index, flag)?))
            }
            _ => return Err(format!("unknown load-client option {flag:?}").into()),
        }
    }
    Ok((
        LoadOptions {
            scenario: scenario.ok_or("load-client requires --scenario")?,
            target: target.ok_or("load-client requires --target")?,
            api_kind: api_kind.ok_or("load-client requires --api-format")?,
            streamed,
            concurrency: concurrency.ok_or("load-client requires --concurrency")?,
            warmup_seconds: warmup_seconds.ok_or("load-client requires --warmup-seconds")?,
            duration_seconds: duration_seconds.ok_or("load-client requires --duration-seconds")?,
            timeout_seconds: timeout_seconds.ok_or("load-client requires --timeout-seconds")?,
            api_key: api_key.ok_or("load-client requires --api-key")?,
            model: model.ok_or("load-client requires --model")?,
        },
        output.ok_or("load-client requires --output")?,
    ))
}

fn required_value<'a>(
    arguments: &'a [String],
    index: &mut usize,
    flag: &str,
) -> Result<&'a str, Box<dyn Error + Send + Sync>> {
    let value = arguments
        .get(*index)
        .ok_or_else(|| format!("{flag} requires a value"))?;
    *index += 1;
    Ok(value)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("performance tool must live under tools/forwarding-perf")
        .to_path_buf()
}

fn print_usage() {
    eprintln!(
        "\
Usage:
  ai-gateway-perf run [--profile quick|standard]
      [--database-admin-url URL] [--gateway-bin PATH]
      [--report-dir PATH] [--keep-database]

Internal commands used by the orchestrator:
  ai-gateway-perf mock-upstream --listen ADDRESS
  ai-gateway-perf load-client --scenario NAME --target URL
      --api-format chat|responses [--stream] --concurrency N
      --warmup-seconds N --duration-seconds N --timeout-seconds N
      --api-key KEY --model MODEL --output PATH
"
    );
}

#[cfg(test)]
mod tests {
    use super::{parse_run_options, repo_root};
    use crate::scenario::ProfileName;

    #[test]
    fn run_options_are_manual_and_default_to_quick() {
        let options = parse_run_options(&[]).unwrap();
        assert_eq!(options.profile, ProfileName::Quick);
        assert!(options.gateway_bin.starts_with(repo_root()));
        assert!(!options.keep_database);
    }

    #[test]
    fn run_options_accept_explicit_profile_and_keep_database() {
        let options = parse_run_options(&[
            "--profile".into(),
            "standard".into(),
            "--keep-database".into(),
        ])
        .unwrap();
        assert_eq!(options.profile, ProfileName::Standard);
        assert!(options.keep_database);
    }
}
