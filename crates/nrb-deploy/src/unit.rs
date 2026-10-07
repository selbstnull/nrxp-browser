//! systemd-юнит узла. Усиление: отдельный пользователь, никаких привилегий кроме привязки к порту <1024, файловая система только
//! для чтения кроме каталога узла, фильтр системных вызовов. Логи трафика на диск не пишутся: stdout в никуда, stderr — в журнал
//! только как есть (ошибки, не клиенты).

pub const SERVICE_NAME: &str = "nrb-node";
pub const INSTALL_DIR: &str = "/opt/nrb";
pub const BINARY_PATH: &str = "/opt/nrb/nrb";
pub const DATA_DIR: &str = "/var/lib/nrb-node";
pub const SERVICE_USER: &str = "nrb-node";

pub fn service_unit() -> String {
    format!(
        "[Unit]\n\
Description=netrunner-browser node (NRXP ingress)\n\
Documentation=https://github.com/nineAp/nrxp-browser\n\
After=network-online.target\n\
Wants=network-online.target\n\
\n\
[Service]\n\
Type=simple\n\
User={SERVICE_USER}\n\
Group={SERVICE_USER}\n\
ExecStart={BINARY_PATH} node run --dir {DATA_DIR}\n\
Restart=always\n\
RestartSec=2\n\
LimitNOFILE=1048576\n\
# Усиление: узлу не нужно почти ничего\n\
NoNewPrivileges=true\n\
CapabilityBoundingSet=CAP_NET_BIND_SERVICE\n\
AmbientCapabilities=CAP_NET_BIND_SERVICE\n\
ProtectSystem=strict\n\
ReadWritePaths={DATA_DIR}\n\
ProtectHome=true\n\
PrivateTmp=true\n\
PrivateDevices=true\n\
ProtectKernelTunables=true\n\
ProtectKernelModules=true\n\
ProtectControlGroups=true\n\
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX\n\
RestrictNamespaces=true\n\
LockPersonality=true\n\
SystemCallFilter=@system-service\n\
SystemCallArchitectures=native\n\
UMask=0077\n\
# Приватность: ничего не пишем в stdout (там мог бы оказаться разбор соединений), ошибки — в журнал.\n\
StandardOutput=null\n\
StandardError=journal\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_is_hardened_and_runs_unprivileged() {
        let unit = service_unit();
        for must in [
            "User=nrb-node",
            "NoNewPrivileges=true",
            "ProtectSystem=strict",
            "ReadWritePaths=/var/lib/nrb-node",
            "StandardOutput=null",
            "CAP_NET_BIND_SERVICE",
            "Restart=always",
        ] {
            assert!(unit.contains(must), "в юните нет {must}");
        }
        assert!(!unit.contains("User=root"));
        assert!(unit.contains("ExecStart=/opt/nrb/nrb node run --dir /var/lib/nrb-node"));
    }
}
