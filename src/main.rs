use anyhow::Result;
use chrono::prelude::*;
use reqwest::blocking::Client;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{collections::HashMap, io::Stdout};
use std::{io, thread};
use structopt::StructOpt;
use termion::raw::{IntoRawMode, RawTerminal};
use threadpool::ThreadPool;
use tui::widgets::{Block, Borders, Gauge};
use tui::Terminal;
use tui::{
    backend::TermionBackend,
    layout::Direction,
    widgets::{Row, Table},
};
use tui::{
    layout::{Constraint, Layout},
    widgets::BorderType,
};
use tui::{
    style::{Color, Modifier, Style},
    text::Text,
};

const MAX_THREADS: usize = 50;

#[derive(Debug, StructOpt)]
#[structopt(name = "paine", about = "What about about?")]
struct TestPlan {
    #[structopt(short, long, help = "target url")]
    url: String,

    #[structopt(short, long, default_value = "10", help = "requests per seconds")]
    rate: u16,

    #[structopt(short, long, default_value = "60", help = "test duration in seconds")]
    duration: usize,

    #[structopt(short, long, default_value = "10", help = "http timeout in seconds")]
    timeout_secs: u64,

    #[structopt(skip)]
    response_times: Vec<u128>,
    #[structopt(skip)]
    status_codes: HashMap<u16, usize>,
    #[structopt(skip)]
    connect_errors: usize,
    #[structopt(skip)]
    timeout_errors: usize,
    #[structopt(skip)]
    other_errors: usize,
    #[structopt(skip)]
    total_elapsed: Duration,
    #[structopt(skip)]
    total_requests: usize,
    #[structopt(skip)]
    date: String,
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
        self.total_requests
    }
    fn total_success(&self) -> usize {
        self.response_times.len()
    }

    fn draw_terminal(
        &self,
        terminal: &mut Terminal<TermionBackend<RawTerminal<Stdout>>>,
        ratio: &f64,
    ) -> Result<()> {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints(
                    [
                        Constraint::Length(3),
                        Constraint::Length(12),
                        Constraint::Min(0),
                    ]
                    .as_ref(),
                )
                .split(f.size());

            let gauge = Gauge::default()
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Plain)
                        .title("Progress"),
                )
                .gauge_style(Style::default().fg(Color::Green))
                .ratio(*ratio);
            f.render_widget(gauge, chunks[0]);

            let total_requests = self.total_requests().to_string();
            let rate = self.rate.to_string();

            let mut status_codes = String::from("");
            for (code, cnt) in self.status_codes.iter() {
                status_codes.push_str(&format!(", \"{}\": {}", code, cnt));
            }

            let (avg, min, max) = self.response_avg_min_max();
            let avg_min_max = format!("Avg: {}ms  Min: {}ms  Max: {}ms", avg, min, max);

            let errors = if self.total_errors() > 0 {
                format!(
                    "{:.1}% ({}/{}) (Connection: {}  Timeouts: {}  Others: {})",
                    self.total_errors() / self.total_requests() * 100,
                    self.total_errors(),
                    self.total_requests(),
                    self.connect_errors,
                    self.timeout_errors,
                    self.other_errors,
                )
            } else {
                "0".to_string()
            };

            let throughput = format!(
                "{:.1} req/s",
                (self.total_success() as f64) / (self.total_elapsed.as_secs_f64() as f64)
            );
            let runtime = format!("{:.1}s", self.total_elapsed.as_secs_f64());
            let success = if self.total_success() > 0 {
                format!(
                    "{:.1}% ({}/{})",
                    self.total_success() / self.total_requests() * 100,
                    self.total_success(),
                    self.total_requests(),
                )
            } else {
                "0".to_string()
            };

            let througput_style = Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD);
            let success_style = if self.total_success() == self.total_requests() {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Yellow)
            };
            let error_style = if self.total_errors() > 0 {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let bold_style = Style::default().add_modifier(Modifier::BOLD);
            let table = Table::new(vec![
                Row::new(vec!["Date", &self.date]),
                Row::new(vec![
                    Text::styled("Target", bold_style),
                    Text::styled(self.url.clone(), bold_style),
                ]),
                Row::new(vec!["Rate", &rate]),
                Row::new(vec!["Runtime", &runtime]),
                Row::new(vec!["Requests", &total_requests]),
                Row::new(vec![
                    Text::styled("Success", success_style),
                    Text::styled(success, success_style),
                ]),
                Row::new(vec![
                    Text::styled("Errors", error_style),
                    Text::styled(errors, error_style),
                ]),
                Row::new(vec!["Status codes", status_codes.trim_start_matches(", ")]),
                Row::new(vec!["Response times", &avg_min_max]),
                Row::new(vec![
                    Text::styled("Throughput", througput_style),
                    Text::styled(throughput, througput_style),
                ]),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .title("Test Report"),
            )
            .widths(&[Constraint::Length(20), Constraint::Length(70)]);
            f.render_widget(table, chunks[1]);
        })?;
        Ok(())
    }
}

enum Response {
    Error(u16),
    Success(u128, u16),
    TimeoutError,
    ConnectionError,
    OtherError,
}

fn do_requests(
    client: Client,
    url: &str,
    rate: u16,
    total_duration: Duration,
    tx: Sender<Response>,
) {
    let sleep_ms = Duration::from_secs_f64(1.0 / rate as f64);
    let pool = ThreadPool::new(std::cmp::min(rate as usize, MAX_THREADS));

    let started = Instant::now();
    while Instant::now().duration_since(started) < total_duration {
        let url = url.to_owned();
        let tx = tx.clone();
        let client = client.clone();
        pool.execute(move || {
            let req_started = Instant::now();
            match client.get(url).send() {
                Ok(response) => {
                    if response.status().is_success() {
                        tx.send(Response::Success(
                            req_started.elapsed().as_millis(),
                            response.status().as_u16(),
                        ))
                        .expect("send0 failed");
                    } else {
                        tx.send(Response::Error(response.status().into()))
                            .expect("send0 failed");
                    }
                }
                Err(e) => {
                    if e.is_timeout() {
                        tx.send(Response::TimeoutError).expect("send0 failed");
                    } else if e.is_connect() {
                        tx.send(Response::ConnectionError).expect("send0 failed");
                    } else {
                        tx.send(Response::OtherError).expect("send0 failed");
                    }
                }
            }
        });
        std::thread::sleep(sleep_ms);
    }
    drop(tx);
}

fn handle_results(data: Arc<Mutex<TestPlan>>, rx: Receiver<Response>) {
    for msg in rx.iter() {
        let mut plan = data.lock().unwrap();
        plan.total_requests += 1;

        match msg {
            Response::Success(millis, code) => {
                plan.response_times.push(millis);
                let entry = plan.status_codes.entry(code).or_insert(0);
                *entry += 1;
            }
            Response::Error(code) => {
                plan.other_errors += 1;
                let entry = plan.status_codes.entry(code).or_insert(0);
                *entry += 1;
            }
            Response::OtherError => plan.other_errors = plan.other_errors + 1,
            Response::ConnectionError => plan.connect_errors = plan.connect_errors + 1,
            Response::TimeoutError => plan.timeout_errors = plan.timeout_errors + 1,
        }
    }
}

fn main() -> Result<()> {
    let mut plan = TestPlan::from_args();
    if plan.rate <= 0 {
        anyhow::bail!("<rate> must be greated than 0.");
    }
    if plan.timeout_secs <= 0 {
        anyhow::bail!("<timeout> must be greated than 0.");
    }
    plan.date = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let rate = plan.rate;
    let url = plan.url.clone();
    let timeout_secs = plan.timeout_secs;
    let duration = Duration::from_secs(plan.duration as u64);

    // run response handler
    let data = Arc::new(Mutex::new(plan));
    let data2 = data.clone();
    let (tx, rx) = channel();
    let handle_results = thread::spawn(move || handle_results(data2, rx));

    // run request executor
    let tx2 = tx.clone();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .expect("unable to create http client");
    let handle_do = thread::spawn(move || do_requests(client, &url, rate, duration, tx2));

    // prepare terminal
    let stdout = io::stdout().into_raw_mode()?;
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // run the main loop
    let started = Instant::now();
    loop {
        let now = Instant::now();
        let mut ratio = now.duration_since(started).as_secs_f64() / duration.as_secs_f64();
        if ratio >= 1.0 {
            ratio = 1.0;
        }
        let mut plan = data.lock().unwrap();
        plan.total_elapsed = now.duration_since(started);
        plan.draw_terminal(&mut terminal, &ratio)?;
        drop(plan);

        if ratio >= 1.0 {
            drop(tx);
            break;
        }

        thread::sleep(Duration::from_secs_f64(0.2));
    }

    handle_do.join().expect("do join failed");
    handle_results.join().expect("results join failed");

    // let _ = stdin().keys().next();
    terminal.set_cursor(0, 16)?;

    Ok(())
}
