//! `nrb corpus` — прогон корпуса сайтов и отчёт совместимости (I3).
//!
//! Каждый сайт открывается отдельным процессом `nrb snap --json`: зависший/упавший сайт не ломает прогон,
//! память не копится, а параллельность получается обычными потоками. Сам оркестратор от Servo не зависит.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::snap::SnapReport;

pub struct Args {
    pub sites: PathBuf,
    pub known_broken: PathBuf,
    pub baseline: PathBuf,
    pub out: Option<PathBuf>,
    pub category: Vec<String>,
    pub filter: Option<String>,
    pub parallel: usize,
    pub timeout: Duration,
    pub proxy: Option<String>,
    pub node: crate::tunnel::NodeArgs,
    pub no_ratchet: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Site {
    id: String,
    category: String,
    url: String,
    expect: Option<String>,
}

#[derive(Deserialize)]
struct SiteFile {
    site: Vec<Site>,
}

#[derive(Deserialize, Default)]
struct BrokenFile {
    #[serde(default)]
    broken: Vec<Broken>,
}

#[derive(Deserialize)]
struct Broken {
    id: String,
    reason: String,
}

#[derive(Deserialize)]
struct Baseline {
    min_pass_percent: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Загрузилась, не пустая, подпись (если задана) совпала.
    Ok,
    /// Отрисовалась (не пустая), но не дозагрузилась до `Complete` — кадр взят из того, что успело нарисоваться.
    Partial,
    /// Загрузилась, но заголовок не содержит ожидаемой подписи.
    SignatureMiss,
    /// Кадр одного цвета.
    Blank,
    /// Движок показал свою страницу сетевой ошибки.
    NetError,
    Timeout,
    Crash,
    Error,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Partial => "partial",
            Status::SignatureMiss => "signature_miss",
            Status::Blank => "blank",
            Status::NetError => "net_error",
            Status::Timeout => "timeout",
            Status::Crash => "crash",
            Status::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SiteResult {
    id: String,
    category: String,
    url: String,
    status: Status,
    wall_ms: u64,
    load_ms: Option<u64>,
    title: Option<String>,
    final_url: Option<String>,
    dominant: Option<f64>,
    console_total: usize,
    console_sample: Vec<String>,
    detail: Option<String>,
    known_broken: Option<String>,
    screenshot: Option<String>,
}

/// Классификация результата `nrb snap`. Чистая функция — покрыта тестами.
fn classify(
    exit: Option<i32>,
    report: Option<&SnapReport>,
    expect: Option<&str>,
    killed: bool,
) -> Status {
    if killed {
        return Status::Timeout;
    }
    match (exit, report) {
        (Some(0 | 4), Some(report)) => {
            let engine_error_page =
                report.final_url.as_deref().is_some_and(|u| {
                    u.starts_with("about:neterror") || u.starts_with("about:error")
                }) || report.title.as_deref().is_some_and(is_error_page_title);
            if engine_error_page {
                Status::NetError
            } else if report.blank {
                Status::Blank
            } else if report.partial {
                Status::Partial
            } else if let Some(expect) = expect.filter(|e| !e.is_empty()) {
                let title = report.title.as_deref().unwrap_or("").to_lowercase();
                if title.contains(&expect.to_lowercase()) {
                    Status::Ok
                } else {
                    Status::SignatureMiss
                }
            } else {
                Status::Ok
            }
        }
        (Some(2), _) => Status::Timeout,
        // Нет отчёта: процесс упал сигналом (exit == None) или вышел с ошибкой движка.
        (None, _) => Status::Crash,
        (Some(3), _) => Status::Crash,
        _ => Status::Error,
    }
}

/// «Сайт открывается»: страница что-то нарисовала и это не страница ошибки.
fn opens(status: Status) -> bool {
    matches!(status, Status::Ok | Status::Partial | Status::SignatureMiss)
}

/// Заголовки страниц ошибок: наши (русские, `nrb-engine-servo/resources`) и стандартная английская Servo.
fn is_error_page_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    lower.contains("error loading page")
        || lower.contains("certificate error")
        || [
            "не удалось открыть страницу",
            "ошибка сертификата",
            "вкладка завершилась аварийно",
        ]
        .iter()
        .any(|known| lower.contains(known))
}

fn percent(passed: usize, total: usize) -> f64 {
    if total == 0 {
        100.0
    } else {
        passed as f64 * 100.0 / total as f64
    }
}

fn run_site(exe: &Path, site: &Site, out: &Path, args: &Args, proxy: Option<&str>) -> SiteResult {
    let png = out.join(format!("{}.png", site.id));
    let mut command = Command::new(exe);
    command
        .args(["snap", &site.url, "-o"])
        .arg(&png)
        .args([
            "--json",
            "--timeout",
            &format!("{}s", args.timeout.as_secs()),
        ])
        .stdout(Stdio::piped())
        .stderr(
            fs::File::create(out.join(format!("{}.log", site.id)))
                .map_or_else(|_| Stdio::null(), Stdio::from),
        );
    // Прокси (с логином/паролем) передаём через окружение, а не через argv: аргументы видны в `ps`.
    if let Some(proxy) = proxy {
        command.env("NRB_PROXY_URL", proxy);
    }
    let started = Instant::now();
    let mut result = SiteResult {
        id: site.id.clone(),
        category: site.category.clone(),
        url: site.url.clone(),
        status: Status::Error,
        wall_ms: 0,
        load_ms: None,
        title: None,
        final_url: None,
        dominant: None,
        console_total: 0,
        console_sample: Vec::new(),
        detail: None,
        known_broken: None,
        screenshot: None,
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            result.detail = Some(format!("не удалось запустить {}: {error}", exe.display()));
            return result;
        }
    };

    // Жёсткий потолок: таймаут движка + запас на старт и скриншот.
    let hard_deadline = started + args.timeout + Duration::from_secs(25);
    let mut killed = false;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() >= hard_deadline => {
                let _ = child.kill();
                let _ = child.wait();
                killed = true;
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                result.detail = Some(format!("wait: {error}"));
                break Some(-1);
            }
        }
    };
    result.wall_ms = started.elapsed().as_millis() as u64;

    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    let report: Option<SnapReport> = stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok());
    result.status = classify(exit, report.as_ref(), site.expect.as_deref(), killed);
    if let Some(report) = report {
        result.load_ms = Some(report.load_ms);
        result.title = report.title;
        result.final_url = report.final_url;
        result.dominant = Some(report.dominant);
        result.console_total = report.console_total;
        result.console_sample = report.console.into_iter().take(5).collect();
        if png.exists() {
            result.screenshot = png.file_name().map(|n| n.to_string_lossy().into_owned());
        }
    }
    if matches!(result.status, Status::Crash | Status::Error) {
        result.detail = Some(format!("код выхода: {exit:?}"));
    }
    result
}

fn timestamp_dir() -> String {
    humantime::format_rfc3339_seconds(SystemTime::now())
        .to_string()
        .replace([':', 'T'], "-")
        .trim_end_matches('Z')
        .to_owned()
}

pub fn run(args: Args) -> i32 {
    let sites: Vec<Site> = match fs::read_to_string(&args.sites)
        .map_err(|e| e.to_string())
        .and_then(|t| toml::from_str::<SiteFile>(&t).map_err(|e| e.to_string()))
    {
        Ok(file) => file.site,
        Err(error) => {
            eprintln!("не удалось прочитать {}: {error}", args.sites.display());
            return 64;
        }
    };
    let broken: BTreeMap<String, String> = fs::read_to_string(&args.known_broken)
        .ok()
        .and_then(|t| toml::from_str::<BrokenFile>(&t).ok())
        .unwrap_or_default()
        .broken
        .into_iter()
        .map(|b| (b.id, b.reason))
        .collect();
    let baseline: f64 = fs::read_to_string(&args.baseline)
        .ok()
        .and_then(|t| serde_json::from_str::<Baseline>(&t).ok())
        .map_or(0.0, |b| b.min_pass_percent);

    let sites: Vec<Site> = sites
        .into_iter()
        .filter(|s| args.category.is_empty() || args.category.contains(&s.category))
        .filter(|s| {
            args.filter
                .as_ref()
                .is_none_or(|f| s.id.contains(f.as_str()))
        })
        .collect();
    if sites.is_empty() {
        eprintln!("после фильтров не осталось сайтов");
        return 64;
    }

    let out = args
        .out
        .clone()
        .unwrap_or_else(|| Path::new("out/corpus").join(timestamp_dir()));
    if let Err(error) = fs::create_dir_all(&out) {
        eprintln!("не удалось создать {}: {error}", out.display());
        return 64;
    }
    if args.node.hermetic {
        eprintln!(
            "corpus: --hermetic не поддерживается (дочерние снимки получают прокси по TCP); проверяйте `nrb snap --hermetic`"
        );
        return 64;
    }
    // Один туннель на весь прогон: все дочерние `nrb snap` ходят через общий локальный прокси.
    let tunnel = if args.node.enabled() {
        match crate::tunnel::start(&args.node) {
            Ok(tunnel) => Some(tunnel),
            Err(error) => {
                eprintln!("туннель: {error}");
                return 64;
            }
        }
    } else {
        None
    };
    let proxy_url = tunnel
        .as_ref()
        .map(|t| t.proxy_url())
        .or_else(|| args.proxy.clone());
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("current_exe: {error}");
            return 64;
        }
    };

    println!(
        "корпус: {} сайтов, параллельность {}, таймаут {:?}, отчёт → {}",
        sites.len(),
        args.parallel,
        args.timeout,
        out.display()
    );
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::<SiteResult>::new());
    std::thread::scope(|scope| {
        for _ in 0..args.parallel.max(1) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(site) = sites.get(index) else { break };
                    let mut result = run_site(&exe, site, &out, &args, proxy_url.as_deref());
                    result.known_broken = broken.get(&site.id).cloned();
                    println!(
                        "[{:>2}/{}] {:<16} {:<14} {:>6} ms  {}",
                        index + 1,
                        sites.len(),
                        site.id,
                        result.status.label(),
                        result.wall_ms,
                        result.title.as_deref().unwrap_or("")
                    );
                    results.lock().expect("mutex").push(result);
                }
            });
        }
    });
    let mut results = results.into_inner().expect("mutex");
    results.sort_by(|a, b| (&a.category, &a.id).cmp(&(&b.category, &b.id)));

    let counted: Vec<&SiteResult> = results
        .iter()
        .filter(|r| r.known_broken.is_none())
        .collect();
    let passed = counted.iter().filter(|r| r.status == Status::Ok).count();
    let pass_percent = percent(passed, counted.len());
    let opened = counted.iter().filter(|r| opens(r.status)).count();

    let _ = fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&results).unwrap_or_default(),
    );
    let _ = fs::write(
        out.join("report.md"),
        render_markdown(&results, pass_percent, baseline),
    );
    println!(
        "\nитого: {passed}/{total} ok ({pass_percent:.1}%); открылось (ok+partial+signature_miss): {opened}/{total} ({:.1}%); baseline {baseline:.1}%  → {}",
        percent(opened, counted.len()),
        out.join("report.md").display(),
        total = counted.len(),
    );

    if !args.no_ratchet && pass_percent + f64::EPSILON < baseline {
        eprintln!("RATCHET: {pass_percent:.1}% ниже baseline {baseline:.1}%");
        return 1;
    }
    0
}

fn render_markdown(results: &[SiteResult], pass_percent: f64, baseline: f64) -> String {
    let mut md = String::new();
    md.push_str("# Отчёт корпуса совместимости\n\n");
    let counted = results.iter().filter(|r| r.known_broken.is_none()).count();
    let passed = results
        .iter()
        .filter(|r| r.known_broken.is_none() && r.status == Status::Ok)
        .count();
    let opened = results
        .iter()
        .filter(|r| r.known_broken.is_none() && opens(r.status))
        .count();
    md.push_str(&format!(
        "**ok: {passed}/{counted} ({pass_percent:.1}%)**; **открылось** (ok + partial + signature_miss): {opened}/{counted} ({:.1}%); baseline {baseline:.1}%. Известные нерабочие: {}.\n\n",
        percent(opened, counted),
        results.len() - counted
    ));

    let mut by_status: BTreeMap<&str, usize> = BTreeMap::new();
    for r in results {
        *by_status.entry(r.status.label()).or_default() += 1;
    }
    md.push_str("## По статусам\n\n| статус | сайтов |\n|---|---|\n");
    for (status, count) in &by_status {
        md.push_str(&format!("| {status} | {count} |\n"));
    }

    let mut by_category: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for r in results.iter().filter(|r| r.known_broken.is_none()) {
        let entry = by_category.entry(&r.category).or_default();
        entry.1 += 1;
        if r.status == Status::Ok {
            entry.0 += 1;
        }
    }
    md.push_str("\n## По категориям\n\n| категория | ok | всего | % |\n|---|---|---|---|\n");
    for (category, (ok, total)) in &by_category {
        md.push_str(&format!(
            "| {category} | {ok} | {total} | {:.0} |\n",
            percent(*ok, *total)
        ));
    }

    md.push_str("\n## Сайты\n\n| id | категория | статус | загрузка, мс | заголовок | консоль | примечание |\n|---|---|---|---|---|---|---|\n");
    for r in results {
        let title = r.title.as_deref().unwrap_or("").replace('|', "\\|");
        let note = r
            .known_broken
            .as_deref()
            .map(|why| format!("известно: {why}"))
            .or_else(|| r.detail.clone())
            .unwrap_or_default();
        md.push_str(&format!(
            "| [{}]({}) | {} | {} | {} | {} | {} | {} |\n",
            r.id,
            r.url,
            r.category,
            r.status.label(),
            r.load_ms.map_or("—".into(), |ms| ms.to_string()),
            title,
            r.console_total,
            note
        ));
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(blank: bool, title: Option<&str>, final_url: Option<&str>) -> SnapReport {
        SnapReport {
            width: 1280,
            height: 800,
            load_ms: 100,
            dominant: if blank { 1.0 } else { 0.5 },
            blank,
            partial: false,
            title: title.map(Into::into),
            final_url: final_url.map(Into::into),
            console: vec![],
            console_total: 0,
        }
    }

    #[test]
    fn ok_requires_signature_when_given() {
        let r = report(
            false,
            Some("Hacker News"),
            Some("https://news.ycombinator.com/"),
        );
        assert_eq!(
            classify(Some(0), Some(&r), Some("hacker news"), false),
            Status::Ok
        );
        assert_eq!(
            classify(Some(0), Some(&r), Some("Reddit"), false),
            Status::SignatureMiss
        );
        assert_eq!(classify(Some(0), Some(&r), None, false), Status::Ok);
        assert_eq!(classify(Some(0), Some(&r), Some(""), false), Status::Ok);
    }

    #[test]
    fn blank_and_neterror_win_over_signature() {
        assert_eq!(
            classify(Some(4), Some(&report(true, None, None)), Some("x"), false),
            Status::Blank
        );
        let net = report(true, None, Some("about:neterror?e=dnsNotFound"));
        assert_eq!(classify(Some(4), Some(&net), None, false), Status::NetError);
    }

    #[test]
    fn partial_and_error_title() {
        let mut partial = report(false, Some("GitHub"), None);
        partial.partial = true;
        assert_eq!(
            classify(Some(0), Some(&partial), Some("github"), false),
            Status::Partial
        );
        let err = report(true, Some("Error loading page"), None);
        assert_eq!(classify(Some(4), Some(&err), None, false), Status::NetError);
        let ru = report(false, Some("Не удалось открыть страницу"), None);
        assert_eq!(classify(Some(0), Some(&ru), None, false), Status::NetError);
        let cert = report(false, Some("Ошибка сертификата"), None);
        assert_eq!(
            classify(Some(0), Some(&cert), Some("example"), false),
            Status::NetError
        );
        assert!(
            opens(Status::Partial)
                && opens(Status::Ok)
                && !opens(Status::Blank)
                && !opens(Status::NetError)
        );
    }

    #[test]
    fn exit_codes_map_to_statuses() {
        assert_eq!(classify(Some(2), None, None, false), Status::Timeout);
        assert_eq!(classify(Some(3), None, None, false), Status::Crash);
        assert_eq!(classify(None, None, None, false), Status::Crash);
        assert_eq!(classify(None, None, None, true), Status::Timeout);
        assert_eq!(classify(Some(64), None, None, false), Status::Error);
    }

    #[test]
    fn percent_handles_empty() {
        assert_eq!(percent(0, 0), 100.0);
        assert_eq!(percent(1, 4), 25.0);
    }

    #[test]
    fn sites_file_parses() {
        let text = fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/corpus/sites.toml"
        ))
        .expect("sites.toml");
        let file: SiteFile = toml::from_str(&text).expect("корректный toml");
        assert!(
            file.site.len() >= 50,
            "в корпусе должно быть ≥ 50 сайтов, а их {}",
            file.site.len()
        );
        let mut ids: Vec<&str> = file.site.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            file.site.len(),
            "id сайтов должны быть уникальны"
        );
    }
}
