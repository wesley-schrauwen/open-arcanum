//! LAN transport: TCP for messages that must arrive, UDP for those that may be lost.
//!
//! Each message says which it needs ([`LedgerDelta::reliable`], [`PlayerIntent::reliable`]).
//! Reliable ones (joining, character creation, spawns, later combat and shared things) go over
//! one TCP connection per client, in order. Unreliable ones (state syncs, moves, sync acks) go
//! as single UDP datagrams, fire and forget: a lost one is replaced by the next, and nothing
//! waits on it. An unreliable message too large for a datagram goes over TCP instead.
//!
//! The host listens on one port number for both: TCP for connections, UDP for datagrams.
//!
//! Joining: the client connects over TCP and sends [`ToHost::Hello`] with its player id. The
//! host answers [`ToClient::Welcome`] with a random token. The client's datagrams carry its
//! player id and that token; the host takes the first datagram that matches as the client's
//! UDP address (and any later one, if it changes), and sends that player's datagrams there.
//! Until then, unreliable messages to the client are dropped. The client sends a
//! [`ToHost::Ping`] datagram every [`PING_EVERY`] so the host learns its address early. A
//! second connection with the same player id replaces the first. There is no further
//! authentication: on a LAN, players are trusted to send their own id.
//!
//! Encoding: JSON. A TCP frame is a 4-byte little-endian length and the message; a datagram
//! is one message of at most [`MAX_DATAGRAM`] bytes.
//!
//! Each end runs a thread that owns its sockets; the transport handle talks to it over
//! channels, so its methods never block. Dropping a handle sends `Bye` and stops the thread.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use arcanum_sim::{LedgerDelta, PlayerId, PlayerIntent};
use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{ClientTransport, HostTransport, TransportError};

/// Bumped when the wire format changes; the host refuses a client of another version.
pub const PROTOCOL: u32 = 2;
/// The largest datagram sent; larger unreliable messages go over TCP.
pub const MAX_DATAGRAM: usize = 1200;
/// The largest TCP frame accepted.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;
/// How often a client pings over UDP.
pub const PING_EVERY: Duration = Duration::from_secs(1);
/// How long a client waits for the TCP connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a socket thread waits for a datagram before it looks at its queues again.
const POLL: Duration = Duration::from_millis(1);

/// What a client sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToHost {
    /// The first message on the connection: who this client plays, and its wire version.
    Hello { protocol: u32, player: PlayerId },
    Intent(PlayerIntent),
    /// A datagram so the host knows the client's UDP address.
    Ping,
    /// The client is leaving.
    Bye,
}

/// What the host sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToClient {
    /// The answer to `Hello`: the token the client's datagrams must carry.
    Welcome { token: u64 },
    Delta(LedgerDelta),
    /// The host is closing, or refused the client.
    Bye,
}

/// A datagram from a client.
#[derive(Debug, Serialize, Deserialize)]
struct FromClient {
    player: PlayerId,
    token: u64,
    msg: ToHost,
}

/// The encoding of `msg` as a TCP frame.
fn frame(msg: &impl Serialize) -> Vec<u8> {
    let json = serde_json::to_vec(msg).expect("messages always serialise");
    let mut out = (json.len() as u32).to_le_bytes().to_vec();
    out.extend(json);
    out
}

/// One end of a TCP connection, non-blocking, with its buffers.
struct Stream {
    stream: TcpStream,
    input: Vec<u8>,
    output: Vec<u8>,
    closed: bool,
}

impl Stream {
    fn new(stream: TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        Ok(Self { stream, input: Vec::new(), output: Vec::new(), closed: false })
    }

    fn send(&mut self, msg: &impl Serialize) {
        self.output.extend(frame(msg));
    }

    /// Writes what it can and reads what has arrived. Returns the complete messages read.
    fn pump<T: DeserializeOwned>(&mut self) -> Vec<T> {
        while !self.output.is_empty() && !self.closed {
            match self.stream.write(&self.output) {
                Ok(0) => self.closed = true,
                Ok(n) => {
                    self.output.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => self.closed = true,
            }
        }
        let mut buf = [0u8; 16 * 1024];
        while !self.closed {
            match self.stream.read(&mut buf) {
                Ok(0) => self.closed = true,
                Ok(n) => self.input.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => self.closed = true,
            }
        }
        let mut msgs = Vec::new();
        while self.input.len() >= 4 {
            let len = u32::from_le_bytes(self.input[..4].try_into().unwrap()) as usize;
            if len > MAX_FRAME {
                self.closed = true;
                break;
            }
            if self.input.len() < 4 + len {
                break;
            }
            match serde_json::from_slice(&self.input[4..4 + len]) {
                Ok(msg) => msgs.push(msg),
                Err(e) => log::warn!("dropped a message that does not decode: {e}"),
            }
            self.input.drain(..4 + len);
        }
        msgs
    }

    /// Sends what is left, as far as it goes at once, and closes.
    fn close(mut self) {
        let _ = self.stream.set_nonblocking(false);
        let _ = self.stream.set_write_timeout(Some(Duration::from_millis(200)));
        let _ = self.stream.write_all(&self.output);
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// Takes every datagram waiting on `socket`, waiting up to [`POLL`] for the first.
fn receive_datagrams(socket: &UdpSocket, mut each: impl FnMut(&[u8], SocketAddr)) {
    let mut buf = [0u8; 65_536];
    let _ = socket.set_nonblocking(false);
    let _ = socket.set_read_timeout(Some(POLL));
    loop {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => each(&buf[..n], from),
            // A closed port on the other side can surface as a reset (Windows); keep going.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
            Err(_) => return,
        }
        let _ = socket.set_nonblocking(true);
    }
}

fn random_token() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.finish()
}

/// The host's end: a TCP listener and a UDP socket on one port.
pub struct LanHost {
    addr: SocketAddr,
    out: Option<Sender<(PlayerId, LedgerDelta)>>,
    intents: Receiver<(PlayerId, PlayerIntent)>,
    players: Arc<RwLock<HashMap<PlayerId, bool>>>,
    thread: Option<JoinHandle<()>>,
}

/// A client connection on the host.
struct Conn {
    stream: Stream,
    /// Set by its `Hello`.
    player: Option<PlayerId>,
    token: u64,
    udp: Option<SocketAddr>,
}

impl LanHost {
    /// Listens on `addr` (port 0 picks a free one; see [`LanHost::local_addr`]).
    pub fn bind(addr: impl ToSocketAddrs) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let addr = listener.local_addr()?;
        let udp = UdpSocket::bind(addr)?;
        listener.set_nonblocking(true)?;
        let (out_tx, out_rx) = unbounded();
        let (in_tx, in_rx) = unbounded();
        let players = Arc::new(RwLock::new(HashMap::new()));
        let shared = players.clone();
        let thread = std::thread::Builder::new().name("lan host".into()).spawn(move || host_loop(&listener, &udp, &out_rx, &in_tx, &shared))?;
        Ok(Self { addr, out: Some(out_tx), intents: in_rx, players, thread: Some(thread) })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Whether `player` is connected.
    pub fn has_player(&self, player: PlayerId) -> bool {
        self.players.read().unwrap().contains_key(&player)
    }

    /// Whether the host knows `player`'s UDP address (so unreliable messages reach it).
    pub fn has_datagrams(&self, player: PlayerId) -> bool {
        self.players.read().unwrap().get(&player).copied().unwrap_or(false)
    }
}

fn host_loop(listener: &TcpListener, udp: &UdpSocket, out: &Receiver<(PlayerId, LedgerDelta)>, intents: &Sender<(PlayerId, PlayerIntent)>, players: &RwLock<HashMap<PlayerId, bool>>) {
    let mut conns: Vec<Conn> = Vec::new();
    loop {
        // New connections.
        loop {
            match listener.accept() {
                Ok((stream, _)) => match Stream::new(stream) {
                    Ok(stream) => conns.push(Conn { stream, player: None, token: random_token(), udp: None }),
                    Err(e) => log::warn!("cannot take a connection: {e}"),
                },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    log::warn!("accept failed: {e}");
                    break;
                }
            }
        }
        // Datagrams.
        receive_datagrams(udp, |data, from| {
            let Ok(d) = serde_json::from_slice::<FromClient>(data) else { return };
            let Some(conn) = conns.iter_mut().find(|c| c.player == Some(d.player) && c.token == d.token) else { return };
            if conn.udp != Some(from) {
                conn.udp = Some(from);
                players.write().unwrap().insert(d.player, true);
            }
            if let ToHost::Intent(intent) = d.msg {
                let _ = intents.send((d.player, intent));
            }
        });
        // Messages to send.
        loop {
            match out.try_recv() {
                Ok((player, delta)) => {
                    let Some(conn) = conns.iter_mut().find(|c| c.player == Some(player)) else { continue };
                    let msg = ToClient::Delta(delta);
                    if !msg_reliable(&msg) {
                        let data = serde_json::to_vec(&msg).expect("messages always serialise");
                        if data.len() <= MAX_DATAGRAM {
                            if let Some(addr) = conn.udp {
                                let _ = udp.send_to(&data, addr);
                            }
                            continue;
                        }
                    }
                    conn.stream.send(&msg);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    for mut conn in conns.drain(..) {
                        conn.stream.send(&ToClient::Bye);
                        conn.stream.close();
                    }
                    return;
                }
            }
        }
        // Connections: write, read, handle.
        let mut replaced = Vec::new();
        for (i, conn) in conns.iter_mut().enumerate() {
            let msgs: Vec<ToHost> = conn.stream.pump();
            for msg in msgs {
                match (msg, conn.player) {
                    (ToHost::Hello { protocol, player }, None) => {
                        if protocol != PROTOCOL {
                            log::warn!("refused player {}: wire version {protocol}, this host speaks {PROTOCOL}", player.0);
                            conn.stream.send(&ToClient::Bye);
                            conn.stream.closed = true;
                            continue;
                        }
                        conn.player = Some(player);
                        conn.stream.send(&ToClient::Welcome { token: conn.token });
                        players.write().unwrap().insert(player, false);
                        replaced.push((i, player));
                        log::info!("player {} joined", player.0);
                    }
                    (ToHost::Intent(intent), Some(player)) => {
                        let _ = intents.send((player, intent));
                    }
                    (ToHost::Bye, _) => conn.stream.closed = true,
                    _ => {}
                }
            }
        }
        // A newer connection for a player replaces the older one.
        for (newest, player) in replaced {
            for (i, conn) in conns.iter_mut().enumerate() {
                if i != newest && conn.player == Some(player) {
                    conn.player = None;
                    conn.stream.closed = true;
                }
            }
        }
        let (gone, kept): (Vec<Conn>, Vec<Conn>) = conns.into_iter().partition(|c| c.stream.closed);
        conns = kept;
        for conn in gone {
            if let Some(player) = conn.player
                && !conns.iter().any(|c| c.player == Some(player))
            {
                players.write().unwrap().remove(&player);
                log::info!("player {} left", player.0);
            }
            conn.stream.close();
        }
    }
}

fn msg_reliable(msg: &ToClient) -> bool {
    match msg {
        ToClient::Delta(d) => d.reliable(),
        _ => true,
    }
}

impl Drop for LanHost {
    fn drop(&mut self) {
        self.out.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl HostTransport for LanHost {
    fn send_delta(&self, player: PlayerId, delta: LedgerDelta) -> Result<(), TransportError> {
        if !self.has_player(player) {
            return Err(TransportError::UnknownPlayer(player));
        }
        let out = self.out.as_ref().ok_or(TransportError::Disconnected)?;
        out.send((player, delta)).map_err(|_| TransportError::Disconnected)
    }

    fn recv_intents(&self) -> Vec<(PlayerId, PlayerIntent)> {
        self.intents.try_iter().collect()
    }
}

/// A client's end, connected to one host.
pub struct LanClient {
    player: PlayerId,
    host: SocketAddr,
    out: Option<Sender<PlayerIntent>>,
    deltas: Receiver<LedgerDelta>,
    /// The host has welcomed this client.
    welcomed: Arc<AtomicBool>,
    /// The connection is up (or still being made).
    alive: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LanClient {
    /// Connects to the host at `host` as `player`. Nothing is waited for: the connection is
    /// made in the background ([`LanClient::welcomed`], [`LanClient::alive`]).
    pub fn connect(host: impl ToSocketAddrs, player: PlayerId) -> io::Result<Self> {
        let host = host.to_socket_addrs()?.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address"))?;
        let (out_tx, out_rx) = unbounded();
        let (in_tx, in_rx) = unbounded();
        let welcomed = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let (w, a) = (welcomed.clone(), alive.clone());
        let thread = std::thread::Builder::new().name("lan client".into()).spawn(move || {
            if let Err(e) = client_loop(host, player, &out_rx, &in_tx, &w) {
                log::warn!("lost the host at {host}: {e}");
            }
            a.store(false, Ordering::Relaxed);
        })?;
        Ok(Self { player, host, out: Some(out_tx), deltas: in_rx, welcomed, alive, thread: Some(thread) })
    }

    pub fn host(&self) -> SocketAddr {
        self.host
    }

    /// Whether the host has accepted the connection.
    pub fn welcomed(&self) -> bool {
        self.welcomed.load(Ordering::Relaxed)
    }

    /// False once the connection failed or closed.
    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

fn client_loop(host: SocketAddr, player: PlayerId, out: &Receiver<PlayerIntent>, deltas: &Sender<LedgerDelta>, welcomed: &AtomicBool) -> io::Result<()> {
    let mut stream = Stream::new(TcpStream::connect_timeout(&host, CONNECT_TIMEOUT)?)?;
    let local: SocketAddr = if host.is_ipv4() { (Ipv4Addr::UNSPECIFIED, 0).into() } else { (Ipv6Addr::UNSPECIFIED, 0).into() };
    let udp = UdpSocket::bind(local)?;
    stream.send(&ToHost::Hello { protocol: PROTOCOL, player });
    let mut token: Option<u64> = None;
    let mut last_ping: Option<Instant> = None;
    loop {
        let datagram = |msg: ToHost, token: u64| serde_json::to_vec(&FromClient { player, token, msg }).expect("messages always serialise");
        receive_datagrams(&udp, |data, from| {
            if from.port() != host.port() {
                return;
            }
            if let Ok(ToClient::Delta(delta)) = serde_json::from_slice::<ToClient>(data) {
                let _ = deltas.send(delta);
            }
        });
        loop {
            match out.try_recv() {
                Ok(intent) => {
                    if intent.reliable() {
                        stream.send(&ToHost::Intent(intent));
                    } else if let Some(token) = token {
                        let data = datagram(ToHost::Intent(intent.clone()), token);
                        if data.len() <= MAX_DATAGRAM {
                            let _ = udp.send_to(&data, host);
                        } else {
                            stream.send(&ToHost::Intent(intent));
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    stream.send(&ToHost::Bye);
                    stream.close();
                    return Ok(());
                }
            }
        }
        if let Some(token) = token
            && last_ping.is_none_or(|t| t.elapsed() >= PING_EVERY)
        {
            let _ = udp.send_to(&datagram(ToHost::Ping, token), host);
            last_ping = Some(Instant::now());
        }
        for msg in stream.pump::<ToClient>() {
            match msg {
                ToClient::Welcome { token: t } => {
                    token = Some(t);
                    welcomed.store(true, Ordering::Relaxed);
                }
                ToClient::Delta(delta) => {
                    let _ = deltas.send(delta);
                }
                ToClient::Bye => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "the host closed the session")),
            }
        }
        if stream.closed {
            return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "the connection closed"));
        }
    }
}

impl Drop for LanClient {
    fn drop(&mut self) {
        self.out.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl ClientTransport for LanClient {
    fn player(&self) -> PlayerId {
        self.player
    }

    fn send_intent(&self, intent: PlayerIntent) -> Result<(), TransportError> {
        if !self.alive() {
            return Err(TransportError::Disconnected);
        }
        let out = self.out.as_ref().ok_or(TransportError::Disconnected)?;
        out.send(intent).map_err(|_| TransportError::Disconnected)
    }

    fn recv_deltas(&self) -> Vec<LedgerDelta> {
        self.deltas.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use arcanum_formats::map::TileLoc;
    use arcanum_sim::{CharacterRequest, NetId, ActorState};

    use super::*;

    fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn sync(number: u32) -> LedgerDelta {
        LedgerDelta::StateSync { number, time: Default::default(), actors: vec![ActorState { net_id: NetId(0), position: TileLoc { x: 1, y: 2 }, path: vec![], seq: 0 }] }
    }

    #[test]
    fn reliable_messages_go_over_tcp_in_order_and_unreliable_ones_over_udp() {
        let host = LanHost::bind("127.0.0.1:0").unwrap();
        let p = PlayerId(42);
        let client = LanClient::connect(host.local_addr(), p).unwrap();
        client.send_intent(PlayerIntent::Character { seq: 1, request: CharacterRequest::New }).unwrap();
        let got = wait_for(|| Some(host.recv_intents()).filter(|v| !v.is_empty()));
        assert_eq!(got, vec![(p, PlayerIntent::Character { seq: 1, request: CharacterRequest::New })]);
        wait_for(|| client.welcomed().then_some(()));
        assert_eq!(host.send_delta(PlayerId(9), LedgerDelta::Tick(1)), Err(TransportError::UnknownPlayer(PlayerId(9))));

        let reliable: Vec<LedgerDelta> = (0..200).map(|n| LedgerDelta::Despawn { net_id: NetId(n) }).collect();
        for d in &reliable {
            host.send_delta(p, d.clone()).unwrap();
        }
        let mut got = Vec::new();
        wait_for(|| {
            got.extend(client.recv_deltas());
            (got.len() >= reliable.len()).then_some(())
        });
        assert_eq!(got, reliable, "every one, in order");

        // Datagrams flow once the host has the client's address from its ping.
        wait_for(|| host.has_datagrams(p).then_some(()));
        host.send_delta(p, sync(7)).unwrap();
        wait_for(|| client.recv_deltas().contains(&sync(7)).then_some(()));
        client.send_intent(PlayerIntent::SyncAck { number: 7 }).unwrap();
        let got = wait_for(|| Some(host.recv_intents()).filter(|v| !v.is_empty()));
        assert_eq!(got, vec![(p, PlayerIntent::SyncAck { number: 7 })]);

        // An unreliable message too big for a datagram goes over TCP.
        let big = LedgerDelta::StateSync {
            number: 8,
            time: Default::default(),
            actors: (0..50).map(|n| ActorState { net_id: NetId(n), position: TileLoc { x: 100_000, y: 100_000 }, path: vec![TileLoc { x: 100_001, y: 100_001 }; 10], seq: 1 }).collect(),
        };
        host.send_delta(p, big.clone()).unwrap();
        wait_for(|| client.recv_deltas().contains(&big).then_some(()));

        drop(client);
        wait_for(|| (!host.has_player(p)).then_some(()));
    }

    #[test]
    fn a_closing_host_ends_the_connection() {
        let host = LanHost::bind("127.0.0.1:0").unwrap();
        let client = LanClient::connect(host.local_addr(), PlayerId(1)).unwrap();
        wait_for(|| host.has_player(PlayerId(1)).then_some(()));
        drop(host);
        wait_for(|| (!client.alive()).then_some(()));
        assert_eq!(client.send_intent(PlayerIntent::Move { seq: 1, path: vec![] }), Err(TransportError::Disconnected));
    }

    #[test]
    fn an_unreachable_host_is_reported() {
        // Bind and drop to find a port nobody listens on.
        let addr = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let client = LanClient::connect(addr, PlayerId(1)).unwrap();
        wait_for(|| (!client.alive()).then_some(()));
    }

    #[test]
    fn a_second_connection_for_a_player_replaces_the_first() {
        let host = LanHost::bind("127.0.0.1:0").unwrap();
        let first = LanClient::connect(host.local_addr(), PlayerId(5)).unwrap();
        wait_for(|| first.welcomed().then_some(()));
        let second = LanClient::connect(host.local_addr(), PlayerId(5)).unwrap();
        wait_for(|| (!first.alive()).then_some(()));
        host.send_delta(PlayerId(5), LedgerDelta::Tick(3)).unwrap();
        wait_for(|| second.recv_deltas().contains(&LedgerDelta::Tick(3)).then_some(()));
    }
}
