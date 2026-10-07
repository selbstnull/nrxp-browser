//! `nrb adblock …` (I12): списки фильтров блокировщика рекламы — скачать/обновить, показать состояние.

use std::path::PathBuf;
use std::time::Duration;

use clap::Subcommand;
use url::Url;

use crate::fetch;
use crate::profile::ProfileArgs;

/// Списки по умолчанию: имя файла → адрес. Лицензии EasyList (GPLv3/CC BY-SA) — поэтому не вшиты, а скачиваются.
const DEFAULT_LISTS: &[(&str, &str)] = &[
    ("easylist.txt", "https://easylist.to/easylist/easylist.txt"),
    (
        "easyprivacy.txt",
        "https://easylist.to/easylist/easyprivacy.txt",
    ),
];

#[derive(Subcommand)]
pub enum AdblockCommand {
    /// Скачать (обновить) EasyList и EasyPrivacy в каталог фильтров профиля.
    Update {
        #[command(flatten)]
        profile: ProfileArgs,
        /// Скачивать через узел NRXP (конфиг или `nrxp://…`); без него — напрямую.
        #[arg(long)]
        node: Option<PathBuf>,
    },
    /// Показать каталог фильтров и сколько правил в каждом списке.
    Status {
        #[command(flatten)]
        profile: ProfileArgs,
    },
}

pub fn run(command: AdblockCommand) -> i32 {
    match command {
        AdblockCommand::Update { profile, node } => update(&profile, node),
        AdblockCommand::Status { profile } => status(&profile),
    }
}

fn update(profile: &ProfileArgs, node: Option<PathBuf>) -> i32 {
    let Some(dir) = profile.filters_dir() else {
        eprintln!("не удалось определить каталог профиля: задайте --profile");
        return 1;
    };
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("{}: {error}", dir.display());
        return 1;
    }
    let mut failed = false;
    for (name, url) in DEFAULT_LISTS {
        // Сначала во временный файл: оборванная загрузка не должна затирать рабочий список.
        let target = dir.join(name);
        let partial = dir.join(format!("{name}.part"));
        eprintln!("{name}: {url}");
        let code = fetch::run(fetch::Args {
            url: Url::parse(url).expect("адрес списка по умолчанию"),
            system: node.is_none(),
            node: node.clone(),
            node_addr: None,
            output: Some(partial.clone()),
            include: false,
            timeout: Duration::from_secs(120),
        });
        if code == 0 && std::fs::rename(&partial, &target).is_ok() {
            eprintln!("  обновлён: {}", target.display());
        } else {
            let _ = std::fs::remove_file(&partial);
            eprintln!("  не удалось обновить {name} (прежняя копия, если была, оставлена)");
            failed = true;
        }
    }
    i32::from(failed)
}

fn status(profile: &ProfileArgs) -> i32 {
    let Some(dir) = profile.filters_dir() else {
        eprintln!("не удалось определить каталог профиля");
        return 1;
    };
    println!("каталог фильтров: {}", dir.display());
    let mut total = 0;
    let mut found = false;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("txt"))
            .collect();
        files.sort();
        for path in files {
            found = true;
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let rules = nrb_adblock::Blocker::from_lists([text]).rule_count();
            total += rules;
            println!("  {}  — правил: {rules}", path.display());
        }
    }
    if !found {
        println!(
            "списков нет. `nrb adblock update` скачает EasyList и EasyPrivacy; можно положить свои *.txt."
        );
    } else {
        println!("всего правил: {total}");
    }
    0
}
