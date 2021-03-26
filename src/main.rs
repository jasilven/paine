use ansi_term::Colour;
use anyhow::Result;
use chrono::prelude::*;
use reqwest;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::mpsc::{channel, Sender};
use std::time::{Duration, Instant};
use structopt::StructOpt;
use threadpool::ThreadPool;

const MAX_THREADS: usize = 50;
const TIMEOUT: u64 = 10;

#[derive(Debug, StructOpt)]
#[structopt(name = "paine", about = "What about about?")]
struct Opt {
    #[structopt(short, long)]
    verbose: bool,

    #[structopt(short, long, default_value = "10")]
    concurrent: u16,

    #[structopt(short, long, default_value = "5")]
    iterations: u16,

    #[structopt(short, long)]
    url: String,
}

struct TestPlan {
    response_times: Vec<u128>,
    status_codes: HashMap<u16, usize>,
    url: String,
    concurrent: usize,
    iterations: usize,
    connect_errors: usize,
    timeout_errors: usize,
    other_errors: usize,
    total_elapsed: Duration,
    utc: DateTime<Utc>,
    pool: ThreadPool,
}

impl TestPlan {
    fn total_errors(&self) -> usize {
        self.connect_errors + self.timeout_errors + self.other_errors
    }

    fn response_avg_min_max(&self) -> (u128, u128, u128) {
        let avg = if self.response_times.is_empty() {
            0
        } else {
            self.response_times.iter().sum::<u128>() / self.response_times.len() as u128
        };

        let min = *self.response_times.iter().min().unwrap_or(&0);
        let max = *self.response_times.iter().max().unwrap_or(&0);

        (avg, min, max)
    }

    fn total_requests(&self) -> usize {
        self.concurrent * self.iterations
    }
    fn total_success(&self) -> usize {
        self.response_times.len()
    }

    fn run(&mut self) {
        let (tx, rx) = channel();

        let now = Instant::now();
        for _ in 0..self.concurrent {
            let tx = tx.clone();
            let iterations = self.iterations;
            let url = self.url.clone();
            self.pool.execute(move || do_it(&url, iterations, tx));
        }
        for reqres in rx.iter().take(self.total_requests()) {
            match reqres {
                RequestResult::Success(millis, code) => {
                    self.response_times.push(millis);
                    let entry = self.status_codes.entry(code).or_insert(0);
                    *entry += 1;
                }
                RequestResult::ErrorStatus(code) => {
                    self.other_errors = self.other_errors + 1;
                    let entry = self.status_codes.entry(code).or_insert(0);
                    *entry += 1;
                }
                RequestResult::OtherError => self.other_errors = self.other_errors + 1,
                RequestResult::ConnectionError => self.connect_errors = self.connect_errors + 1,
                RequestResult::Timeout => self.timeout_errors = self.timeout_errors + 1,
            }
        }
        self.total_elapsed = now.elapsed();
    }
}

impl From<Opt> for TestPlan {
    fn from(opt: Opt) -> Self {
        TestPlan {
            response_times: vec![],
            status_codes: HashMap::new(),
            url: opt.url,
            concurrent: opt.concurrent as usize,
            iterations: opt.iterations as usize,
            connect_errors: 0,
            timeout_errors: 0,
            other_errors: 0,
            total_elapsed: Duration::new(0, 0),
            utc: Utc::now(),
            pool: ThreadPool::new(std::cmp::min(opt.concurrent.into(), MAX_THREADS)),
        }
    }
}

impl Display for TestPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Date
        write!(
            f,
            "{:>15}: {} \n",
            "Date",
            self.utc.format("%Y-%m-%d %H:%M")
        )?;

        // Target
        let target = format!(
            "{:>15}: {} \n",
            "Target",
            &self.url[0..std::cmp::min(90, self.url.len())]
        );
        f.write_str(&Colour::Cyan.bold().paint(target).to_string())?;

        // Runtime
        write!(
            f,
            "{:>15}: {:.1}s\n",
            "Runtime",
            self.total_elapsed.as_secs_f64()
        )?;
        write!(f, "{:>15}: {} requests\n", "Total", self.total_requests())?;

        // Concurrency
        write!(
            f,
            "{:>15}: {} users, {} iterations/user\n",
            "Concurrency", self.concurrent, self.iterations,
        )?;

        // Status codes
        if !self.status_codes.is_empty() {
            let mut status_codes = String::from("");
            for (code, cnt) in self.status_codes.iter() {
                status_codes.push_str(&format!(", \"{}\": {}", code, cnt));
            }
            write!(
                f,
                "{:>15}: {}\n",
                "Status codes",
                status_codes.trim_start_matches(", ")
            )?;
        }

        if self.total_success() > 0 {
            // Response times
            write!(
                f,
                "{:>15}: Avg: {}ms, Min: {}ms, Max: {}ms\n",
                "Response times",
                self.response_avg_min_max().0,
                self.response_avg_min_max().1,
                self.response_avg_min_max().2,
            )?;

            // Throughput
            let throughput = format!(
                "{:>15}: {:.1} req/s\n",
                "Throughput",
                (self.total_success() as f64) / (self.total_elapsed.as_secs_f64() as f64)
            );
            f.write_str(&Colour::Blue.bold().paint(throughput).to_string())?;

            // Success
            let success = format!(
                "{:>15}: {:.1}% ({}/{})\n",
                "Success",
                (self.total_success() as f64 / self.total_requests() as f64 * 100f64),
                self.total_success(),
                self.total_requests(),
            );

            match self.total_success().cmp(&self.total_requests()) {
                std::cmp::Ordering::Equal => {
                    f.write_str(&Colour::Green.bold().paint(success).to_string())?
                }
                std::cmp::Ordering::Greater => f.write_str(&success)?,
                std::cmp::Ordering::Less => {
                    f.write_str(&Colour::Yellow.paint(success).to_string())?
                }
            };
        }

        // Errors
        if self.total_errors() > 0 {
            let error_prefix = format!(
                "{:>15}: {:.1}% ({}/{}) ",
                "Errors",
                (self.total_errors() as f64 / self.total_requests() as f64 * 100f64),
                self.total_errors(),
                self.total_requests(),
            );

            let mut error_line = Colour::Red.bold().paint(error_prefix).to_string();

            let error_suffix = format!(
                "(Timeouts: {}, Connect: {}, Others: {})\n",
                self.timeout_errors, self.connect_errors, self.other_errors
            );
            error_line.push_str(&error_suffix);
            f.write_str(&error_line)?;
        }

        Ok(())
    }
}

enum RequestResult {
    ErrorStatus(u16),
    Success(u128, u16),
    Timeout,
    ConnectionError,
    OtherError,
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    if opt.concurrent <= 0 {
        anyhow::bail!("<concurrent> must be greated that 0.");
    }
    if opt.iterations <= 0 {
        anyhow::bail!("<iterations> must be greated that 0.");
    }

    let mut plan = TestPlan::from(opt);
    plan.run();
    println!("{}", plan);

    Ok(())
}

fn do_it(url: &str, iterations: usize, tx: Sender<RequestResult>) {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(TIMEOUT))
        .build()
        .expect("unable to create http client");
    for _ in 0..iterations {
        let now = Instant::now();
        match client.get(url).send() {
            Ok(response) => {
                if response.status().is_success() {
                    tx.send(RequestResult::Success(
                        now.elapsed().as_millis(),
                        response.status().as_u16(),
                    ))
                    .expect("send0 failed");
                } else {
                    tx.send(RequestResult::ErrorStatus(response.status().into()))
                        .expect("send0 failed");
                }
            }
            Err(e) => {
                if e.is_timeout() {
                    tx.send(RequestResult::Timeout).expect("send0 failed");
                } else if e.is_connect() {
                    tx.send(RequestResult::ConnectionError)
                        .expect("send0 failed");
                } else {
                    tx.send(RequestResult::OtherError).expect("send0 failed");
                }
            }
        }
    }
}
