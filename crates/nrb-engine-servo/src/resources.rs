//! Собственный `ResourceReader` Servo: ресурсы по умолчанию + наши страницы ошибок на русском.
//! Встроенный ридер Servo отключён (в `Cargo.toml` нет фичи `bundled`) — иначе зарегистрировались бы два.

use std::path::PathBuf;

use servo::resources::{Resource, ResourceReaderMethods};

pub struct NrbResourceReader;

impl ResourceReaderMethods for NrbResourceReader {
    fn sandbox_access_files(&self) -> Vec<PathBuf> {
        vec![]
    }

    fn sandbox_access_files_dirs(&self) -> Vec<PathBuf> {
        vec![]
    }

    fn read(&self, file: Resource) -> Vec<u8> {
        match file {
            Resource::BluetoothBlocklist => &include_bytes!("../resources/gatt_blocklist.txt")[..],
            Resource::DomainList => &include_bytes!("../resources/public_domains.txt")[..],
            Resource::HstsPreloadList => &include_bytes!("../resources/hsts_preload.fstmap")[..],
            Resource::BadCertHTML => &include_bytes!("../resources/badcert.html")[..],
            Resource::NetErrorHTML => &include_bytes!("../resources/neterror.html")[..],
            Resource::BrokenImageIcon => &include_bytes!("../resources/rippy.png")[..],
            Resource::CrashHTML => &include_bytes!("../resources/crash.html")[..],
            Resource::DirectoryListingHTML => {
                &include_bytes!("../resources/directory-listing.html")[..]
            }
            Resource::AboutMemoryHTML => &include_bytes!("../resources/about-memory.html")[..],
            Resource::DebuggerJS => &include_bytes!("../resources/debugger.js")[..],
            Resource::JsonViewerHTML => &include_bytes!("../resources/json-viewer.html")[..],
        }
        .to_owned()
    }
}

servo::submit_resource_reader!(&NrbResourceReader);

/// Заголовки наших страниц ошибок — по ним корпус отличает страницу ошибки от обычной (`nrb corpus`).
pub const ERROR_PAGE_TITLES: [&str; 3] = [
    "Не удалось открыть страницу",
    "Ошибка сертификата",
    "Вкладка завершилась аварийно",
];
