use std::time::Duration;

use async_trait::async_trait;
use nrb_transport::{NrxpTransport, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{DeployError, Result, Verifier};

/// Проверка развёрнутого узла настоящим клиентом с этой машины: хендшейк по приглашению, затем поток наружу
/// (`example.com:80`, HEAD-запрос). Хендшейк обязателен; выход в интернет — предупреждение (у сервера может быть свой файрвол).
pub struct NrxpVerifier {
    /// Обязательно ли доказывать выход в интернет (иначе только предупреждение).
    pub require_egress: bool,
}

#[async_trait]
impl Verifier for NrxpVerifier {
    async fn verify(&self, invite: &str) -> Result<Vec<String>> {
        let invite = nrb_node::Invite::parse(invite).map_err(|e| {
            DeployError::Verify(format!("приглашение проверочного устройства: {e}"))
        })?;
        let mut config = invite.to_node_config();
        config.connect_timeout_secs = 15;
        let transport = NrxpTransport::connect(&config).await.map_err(|e| {
            DeployError::Verify(format!(
                "хендшейк с {}: {e} (порт закрыт файрволом провайдера? служба не запустилась?)",
                config.addr
            ))
        })?;

        let egress = async {
            let mut stream = transport.open_tcp("example.com", 80).await?;
            stream
                .write_all(b"HEAD / HTTP/1.0\r\nHost: example.com\r\nConnection: close\r\n\r\n")
                .await?;
            let mut head = [0u8; 12];
            tokio::time::timeout(Duration::from_secs(15), stream.read_exact(&mut head))
                .await
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
            if !head.starts_with(b"HTTP/") {
                return Err(std::io::Error::other("в ответ пришло не HTTP"));
            }
            Ok::<_, std::io::Error>(())
        }
        .await;
        match egress {
            Ok(()) => Ok(vec![]),
            Err(e) if self.require_egress => Err(DeployError::Verify(format!(
                "узел принял клиента, но выход в интернет не работает: {e}"
            ))),
            Err(e) => Ok(vec![format!(
                "узел принимает клиентов, но выход в интернет не подтверждён ({e}): проверьте исходящий трафик сервера"
            )]),
        }
    }
}
