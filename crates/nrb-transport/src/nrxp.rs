//! Потоки поверх NRXP без TUN: `ClientHandler::connect` + обмен `RawCastFrame`
//! (тот же публичный API, что у `tools/loadtest` и клиентского движка приложения).

use std::io;
use std::net::Ipv4Addr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashMap;
use netrunner_core::net::{ClientHandler, CreditReceiver, GLOBAL_MIN_RTT, Muxer, NetworkConfig};
use netrunner_core::rawcast::{LocalProtocol, RawCastEvent, RawCastFrame};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

use crate::{BoxDatagramFlow, BoxStream, Caps, DatagramFlow, NodeConfig, Transport};

/// Максимальный payload одного кадра: поле длины RawCast — `u16`, берём с запасом, как это делает ядро.
const WRITE_CHUNK: usize = 16 * 1024;
/// Глубина общего канала в туннель: короткая, чтобы кредиты не ждали за длинной очередью чужих Data-кадров.
const FRAME_CHANNEL: usize = 64;
/// MTU, из которого ядро выводит размеры буферов и каналов (тот же, что у реального клиента и loadtest).
const CORE_MTU: usize = 1450;

/// Ядро требует один раз инициализировать глобальную сетевую конфигурацию до первого подключения.
fn init_core() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| NetworkConfig::init_global(CORE_MTU));
}

enum Event {
    Data(Bytes),
    Close,
}

type Registry = Arc<DashMap<u64, mpsc::UnboundedSender<Event>>>;

/// Транспорт через NRXP-сессию (одна на весь браузер; все вкладки — потоки одного `Muxer`).
pub struct NrxpTransport {
    muxer: Arc<Muxer>,
    to_tunnel: mpsc::Sender<RawCastFrame>,
    streams: Registry,
    next_id: AtomicU64,
}

impl NrxpTransport {
    /// Поднимает сессию и ждёт первую ногу (`ClientHandler::connect` возвращается сразу, а `Connect`-кадр,
    /// отправленный до появления ноги, теряется).
    pub async fn connect(config: &NodeConfig) -> io::Result<Self> {
        init_core();
        let identity = config
            .identity()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let (to_tunnel, rx_from_app) = mpsc::channel::<RawCastFrame>(FRAME_CHANNEL);
        let (tx_to_app, mut from_tunnel) = mpsc::channel::<RawCastFrame>(FRAME_CHANNEL);

        let muxer = ClientHandler::connect(
            &config.addr,
            config.decoy_sni.clone(),
            config.auth_token.clone(),
            identity,
            rx_from_app,
            tx_to_app,
        )
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e.to_string()))?;

        let deadline = Instant::now() + config.connect_timeout();
        while muxer.active_legs_count() == 0 {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "узел {} не ответил за {:?}",
                        config.addr,
                        config.connect_timeout()
                    ),
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Диспетчер: кадры из туннеля → поток по `socket_id`.
        let streams: Registry = Arc::new(DashMap::new());
        let dispatch = streams.clone();
        tokio::spawn(async move {
            while let Some(frame) = from_tunnel.recv().await {
                match frame.event {
                    RawCastEvent::Data => {
                        if let Some(stream) = dispatch.get(&frame.socket_id) {
                            let _ = stream.send(Event::Data(frame.payload));
                        }
                    }
                    RawCastEvent::Close => {
                        if let Some((_, stream)) = dispatch.remove(&frame.socket_id) {
                            let _ = stream.send(Event::Close);
                        }
                    }
                    RawCastEvent::Connect | RawCastEvent::Credit => {}
                }
            }
            // Туннель закрыт: все потоки получают EOF.
            dispatch.clear();
        });

        // Сторож: когда пропадают ВСЕ ноги, состояние потоков на узле (возможно, перезапущенном) считается потерянным —
        // закрываем все открытые потоки (клиент получит EOF и повторит запрос), а не оставляем их висеть вечно.
        // Одиночная потеря ноги потоки не трогает: ядро переносит их на живые ноги.
        let watched = Arc::downgrade(&streams);
        let watched_muxer = muxer.clone();
        tokio::spawn(async move {
            let mut was_up = true;
            loop {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let Some(streams) = watched.upgrade() else {
                    return;
                };
                let up = watched_muxer.active_legs_count() > 0;
                if was_up && !up {
                    let ids: Vec<u64> = streams.iter().map(|entry| *entry.key()).collect();
                    tracing::warn!(
                        streams = ids.len(),
                        "все ноги туннеля потеряны — закрываем открытые потоки"
                    );
                    for id in ids {
                        if let Some((_, stream)) = streams.remove(&id) {
                            let _ = stream.send(Event::Close);
                        }
                    }
                }
                was_up = up;
            }
        });

        Ok(Self {
            muxer,
            to_tunnel,
            streams,
            next_id: AtomicU64::new(1),
        })
    }

    /// Мультиплексор сессии — для статистики и диагностики.
    pub fn muxer(&self) -> &Arc<Muxer> {
        &self.muxer
    }
}

#[async_trait]
impl Transport for NrxpTransport {
    async fn open_tcp(&self, host: &str, port: u16) -> io::Result<BoxStream> {
        // Нет ни одной ноги — `Connect` ушёл бы в никуда (и повис бы без ошибки): отказываем сразу.
        if self.muxer.active_legs_count() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "туннель недоступен: нет активных ног",
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        self.streams.insert(id, events_tx);

        // Имя хоста уходит как есть в payload — резолвит выходная нода (DNS-утечки нет).
        let mut connect =
            RawCastFrame::connect(LocalProtocol::Tcp, id, Ipv4Addr::UNSPECIFIED, port);
        connect.payload = Bytes::from(format!("{host}:{port}"));
        let initial_grant = RawCastFrame::credit(
            LocalProtocol::Tcp,
            id,
            Ipv4Addr::UNSPECIFIED,
            port,
            CreditReceiver::initial_offset(),
        );
        let sent = async {
            self.to_tunnel.send(connect).await.ok()?;
            self.to_tunnel.send(initial_grant).await.ok()
        }
        .await;
        if sent.is_none() {
            self.streams.remove(&id);
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "туннель закрыт"));
        }

        Ok(Box::new(NrxpStream {
            id,
            port,
            writer: PollSender::new(self.to_tunnel.clone()),
            control: self.to_tunnel.clone(),
            events: events_rx,
            leftover: Bytes::new(),
            credit: CreditReceiver::new(),
            remote_closed: false,
            local_closed: false,
            registry: self.streams.clone(),
        }))
    }

    async fn open_udp(&self, host: &str, port: u16) -> io::Result<BoxDatagramFlow> {
        if self.muxer.active_legs_count() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "туннель недоступен: нет активных ног",
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        self.streams.insert(id, events_tx);

        // Как и у TCP, имя хоста уходит в payload как есть; у UDP нет кредитов — сразу готово к обмену.
        let mut connect =
            RawCastFrame::connect(LocalProtocol::Udp, id, Ipv4Addr::UNSPECIFIED, port);
        connect.payload = Bytes::from(format!("{host}:{port}"));
        if self.to_tunnel.send(connect).await.is_err() {
            self.streams.remove(&id);
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "туннель закрыт"));
        }
        Ok(Box::new(NrxpDatagrams {
            id,
            port,
            to_tunnel: self.to_tunnel.clone(),
            events: tokio::sync::Mutex::new(events_rx),
            registry: self.streams.clone(),
            // Потолок датаграммной ноги ядра (`MAX_DATAGRAM_LEG_PAYLOAD`) минус заголовки кадра и AEAD — с запасом.
            max_payload: 1100,
        }))
    }

    fn caps(&self) -> Caps {
        Caps {
            native_datagram_leg: self.muxer.datagram_leg_stats().is_some(),
        }
    }

    fn name(&self) -> &'static str {
        "nrxp"
    }
}

/// Один TCP-поток NRXP. Чтение отдаёт кредиты отправителю по мере того, как данные РЕАЛЬНО забирает приложение
/// (так же, как клиентский движок приложения), поэтому медленный читатель не раздувает память и не убивает поток.
pub struct NrxpStream {
    id: u64,
    port: u16,
    writer: PollSender<RawCastFrame>,
    control: mpsc::Sender<RawCastFrame>,
    events: mpsc::UnboundedReceiver<Event>,
    leftover: Bytes,
    credit: CreditReceiver,
    remote_closed: bool,
    local_closed: bool,
    registry: Registry,
}

impl NrxpStream {
    fn frame(&self, event: RawCastEvent, payload: Bytes) -> RawCastFrame {
        RawCastFrame {
            protocol: LocalProtocol::Tcp,
            event,
            socket_id: self.id,
            dst_ip: Ipv4Addr::UNSPECIFIED,
            dst_port: self.port,
            payload,
        }
    }

    /// Отдать отправителю новый абсолютный лимит. Отдельной задачей: общий канал в туннель может быть забит
    /// нашими же Data-кадрами (полнодуплексная перекачка), а читатель после этого засыпает и не смог бы повторить
    /// попытку — получилась бы взаимная блокировка (поток вставал ровно на начальном окне в 1 МБ). Значение
    /// абсолютное и идемпотентное, поэтому поздний кадр ничего не откатывает (так же шлёт управляющие кадры ядро).
    fn send_grant(&self, offset: u32) {
        let frame = RawCastFrame::credit(
            LocalProtocol::Tcp,
            self.id,
            Ipv4Addr::UNSPECIFIED,
            self.port,
            offset,
        );
        let control = self.control.clone();
        tokio::spawn(async move {
            let _ = control.send(frame).await;
        });
    }

    fn on_consumed(&mut self, n: usize) {
        let rtt_ms = GLOBAL_MIN_RTT.load(Ordering::Relaxed);
        let rtt = Duration::from_millis(if rtt_ms == 0 { 100 } else { u64::from(rtt_ms) });
        if let Some(offset) = self.credit.on_consumed(n, Instant::now(), rtt) {
            self.send_grant(offset);
        }
    }
}

impl AsyncRead for NrxpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        loop {
            if !this.leftover.is_empty() {
                let n = this.leftover.len().min(buf.remaining());
                buf.put_slice(&this.leftover.split_to(n));
                this.on_consumed(n);
                return Poll::Ready(Ok(()));
            }
            if this.remote_closed {
                return Poll::Ready(Ok(())); // EOF
            }
            match ready!(this.events.poll_recv(cx)) {
                Some(Event::Data(bytes)) => this.leftover = bytes,
                Some(Event::Close) | None => this.remote_closed = true,
            }
        }
    }
}

impl AsyncWrite for NrxpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        if this.local_closed {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        ready!(this.writer.poll_reserve(cx))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        let n = data.len().min(WRITE_CHUNK);
        let frame = this.frame(RawCastEvent::Data, Bytes::copy_from_slice(&data[..n]));
        this.writer
            .send_item(frame)
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if this.local_closed {
            return Poll::Ready(Ok(()));
        }
        ready!(this.writer.poll_reserve(cx))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        let frame = this.frame(RawCastEvent::Close, Bytes::new());
        this.local_closed = true;
        this.writer
            .send_item(frame)
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Poll::Ready(Ok(()))
    }
}

impl Drop for NrxpStream {
    fn drop(&mut self) {
        self.registry.remove(&self.id);
        if !self.local_closed {
            let close = self.frame(RawCastEvent::Close, Bytes::new());
            if let Err(mpsc::error::TrySendError::Full(close)) = self.control.try_send(close) {
                let control = self.control.clone();
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        let _ = control.send(close).await;
                    });
                }
            }
        }
    }
}

/// UDP-поток NRXP (`UdpConnect` + `UdpData`). Потерянная датаграмма — штатная ситуация, поэтому переполнение очереди
/// в туннель не считается ошибкой.
pub struct NrxpDatagrams {
    id: u64,
    port: u16,
    to_tunnel: mpsc::Sender<RawCastFrame>,
    events: tokio::sync::Mutex<mpsc::UnboundedReceiver<Event>>,
    registry: Registry,
    max_payload: usize,
}

#[async_trait]
impl DatagramFlow for NrxpDatagrams {
    fn send(&self, payload: &[u8]) -> io::Result<()> {
        if payload.len() > self.max_payload {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "датаграмма больше потолка ноги",
            ));
        }
        let frame = RawCastFrame::data(
            LocalProtocol::Udp,
            self.id,
            Ipv4Addr::UNSPECIFIED,
            self.port,
            Bytes::copy_from_slice(payload),
        );
        match self.to_tunnel.try_send(frame) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
        }
    }

    async fn recv(&self) -> io::Result<Bytes> {
        match self.events.lock().await.recv().await {
            Some(Event::Data(bytes)) => Ok(bytes),
            Some(Event::Close) | None => Err(io::Error::from(io::ErrorKind::BrokenPipe)),
        }
    }

    fn max_payload(&self) -> usize {
        self.max_payload
    }
}

impl Drop for NrxpDatagrams {
    fn drop(&mut self) {
        self.registry.remove(&self.id);
        let close = RawCastFrame::close(
            LocalProtocol::Udp,
            self.id,
            Ipv4Addr::UNSPECIFIED,
            self.port,
        );
        if let Err(mpsc::error::TrySendError::Full(close)) = self.to_tunnel.try_send(close)
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let control = self.to_tunnel.clone();
            runtime.spawn(async move {
                let _ = control.send(close).await;
            });
        }
    }
}
