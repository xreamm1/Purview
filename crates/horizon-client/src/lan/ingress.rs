//! Admission before the hub's per-player inbox: a stalled game thread must
//! not let decoded play packets grow the network-to-game channel forever.
use super::NetEvent;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
};

const PACKETS: usize = 32_768;
const BYTES: usize = 64 << 20;
const PLAYER_PACKETS: usize = 8_000;
const PLAYER_BYTES: usize = 16 << 20;
const EVENTS_PER_POLL: usize = 4_096;

#[derive(Default, Clone, Copy)]
struct Usage {
    packets: usize,
    bytes: usize,
}

#[derive(Default)]
struct Budget {
    total: Usage,
    players: HashMap<u32, Usage>,
    peak_packets: usize,
    peak_body_bytes: usize,
    rejected_packets: u64,
}

/// Queue gauges and lifetime high-water marks for `horizon-report`.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct IngressStats {
    pub queued_packets: usize,
    pub queued_body_bytes: usize,
    pub peak_packets: usize,
    pub peak_body_bytes: usize,
    pub rejected_packets: u64,
}

struct Charge {
    budget: Arc<Mutex<Budget>>,
    conn: u32,
    bytes: usize,
}

impl Charge {
    fn reserve(budget: &Arc<Mutex<Budget>>, conn: u32, bytes: usize) -> Option<Self> {
        let mut b = budget.lock().ok()?;
        let player = b.players.get(&conn).copied().unwrap_or_default();
        if b.total.packets >= PACKETS
            || bytes > BYTES.saturating_sub(b.total.bytes)
            || player.packets >= PLAYER_PACKETS
            || bytes > PLAYER_BYTES.saturating_sub(player.bytes)
        {
            b.rejected_packets = b.rejected_packets.saturating_add(1);
            return None;
        }
        b.total.packets += 1;
        b.total.bytes += bytes;
        b.peak_packets = b.peak_packets.max(b.total.packets);
        b.peak_body_bytes = b.peak_body_bytes.max(b.total.bytes);
        let player = b.players.entry(conn).or_default();
        player.packets += 1;
        player.bytes += bytes;
        Some(Self {
            budget: budget.clone(),
            conn,
            bytes,
        })
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        // No user code runs under this lock. Even after poison, releasing a
        // previously admitted packet must not strand its reservation.
        let mut b = self.budget.lock().unwrap_or_else(|e| e.into_inner());
        b.total.packets -= 1;
        b.total.bytes -= self.bytes;
        if let Some(player) = b.players.get_mut(&self.conn) {
            player.packets -= 1;
            player.bytes -= self.bytes;
            if player.packets == 0 {
                b.players.remove(&self.conn);
            }
        }
    }
}

struct Queued {
    event: NetEvent,
    _charge: Option<Charge>,
}

/// Capacity-limited network ingress. Lifecycle messages retain FIFO order with
/// packets and do not consume packet credit: `Left` must survive a flood.
/// Packet bytes count the uncompressed decoded body, not wire bytes or heap size.
#[derive(Clone)]
pub struct EventSender {
    tx: mpsc::Sender<Queued>,
    budget: Arc<Mutex<Budget>>,
}

impl EventSender {
    /// No wait for consumer progress; the short accounting lock may contend.
    pub fn stats(&self) -> Option<IngressStats> {
        let b = self.budget.lock().ok()?;
        Some(IngressStats {
            queued_packets: b.total.packets,
            queued_body_bytes: b.total.bytes,
            peak_packets: b.peak_packets,
            peak_body_bytes: b.peak_body_bytes,
            rejected_packets: b.rejected_packets,
        })
    }

    /// A full packet budget rejects the packet; the connection must close.
    pub fn send(&self, event: NetEvent) -> Result<(), mpsc::TrySendError<NetEvent>> {
        let charge = if let NetEvent::Packet(conn, _, bytes) = &event {
            match Charge::reserve(&self.budget, *conn, *bytes) {
                Some(charge) => Some(charge),
                None => return Err(mpsc::TrySendError::Full(event)),
            }
        } else {
            None
        };
        self.tx
            .send(Queued {
                event,
                _charge: charge,
            })
            .map_err(|e| mpsc::TrySendError::Disconnected(e.0.event))
    }
}

pub(super) struct EventReceiver(mpsc::Receiver<Queued>);

impl EventReceiver {
    pub(super) fn poll(&self) -> Vec<NetEvent> {
        // Never wait for a producer to become idle. Charges remain held by
        // the batch until it has been detached from the channel.
        let batch: Vec<_> = self.0.try_iter().take(EVENTS_PER_POLL).collect();
        batch.into_iter().map(|q| q.event).collect()
    }
}

pub(super) fn channel() -> (EventSender, EventReceiver) {
    let (tx, rx) = mpsc::channel();
    (
        EventSender {
            tx,
            budget: Arc::default(),
        },
        EventReceiver(rx),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::packets::play;

    fn packet(conn: u32, bytes: usize) -> NetEvent {
        NetEvent::Packet(
            conn,
            Box::new(play::Serverbound::Unknown(0, Vec::new())),
            bytes,
        )
    }

    #[test]
    fn a_flood_is_rejected_without_consuming_another_players_credit() {
        let (tx, rx) = channel();
        for _ in 0..PLAYER_PACKETS {
            assert!(tx.send(packet(1, 1)).is_ok());
        }
        assert!(matches!(
            tx.send(packet(1, 1)),
            Err(mpsc::TrySendError::Full(_))
        ));
        assert!(tx.send(packet(2, 1)).is_ok());
        // Closing is ordered after the admitted packets, even at the limit.
        assert!(tx.send(NetEvent::Left(1, String::new())).is_ok());
        assert_eq!(rx.poll().len(), EVENTS_PER_POLL);
        let rest = rx.poll();
        assert!(matches!(rest.last(), Some(NetEvent::Left(1, _))));
        assert!(tx.send(packet(1, 1)).is_ok());
    }

    #[test]
    fn byte_limits_and_global_limits_release_exactly_once() {
        let (tx, rx) = channel();
        for conn in 0..4 {
            assert!(tx.send(packet(conn, PLAYER_BYTES)).is_ok());
            assert!(matches!(
                tx.send(packet(conn, 1)),
                Err(mpsc::TrySendError::Full(_))
            ));
        }
        assert!(matches!(
            tx.send(packet(4, 1)),
            Err(mpsc::TrySendError::Full(_))
        ));
        assert_eq!(rx.poll().len(), 4);
        let stats = tx.stats();
        assert!(matches!(
            stats,
            Some(IngressStats {
                queued_packets: 0,
                queued_body_bytes: 0,
                peak_packets: 4,
                peak_body_bytes: BYTES,
                rejected_packets: 5,
            })
        ));
        assert!(tx.send(packet(4, PLAYER_BYTES)).is_ok());
        drop(rx);
        assert!(matches!(
            tx.send(packet(5, 1)),
            Err(mpsc::TrySendError::Disconnected(_))
        ));
        let b = tx.budget.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!((b.total.packets, b.total.bytes, b.players.len()), (0, 0, 0));
    }

    #[test]
    fn global_count_is_bounded_even_for_empty_packets() {
        let (tx, rx) = channel();
        for i in 0..PACKETS {
            assert!(tx.send(packet((i % 8) as u32, 0)).is_ok());
        }
        assert!(matches!(
            tx.send(packet(9, 0)),
            Err(mpsc::TrySendError::Full(_))
        ));
        assert_eq!(rx.poll().len(), EVENTS_PER_POLL);
        assert!(tx.send(packet(9, 0)).is_ok());
    }

    #[test]
    fn concurrent_producers_keep_their_order_and_deliver_once() {
        let (tx, rx) = channel();
        std::thread::scope(|s| {
            for conn in 0..4 {
                let tx = tx.clone();
                s.spawn(move || {
                    for seq in 0..1_000 {
                        assert!(tx.send(packet(conn, seq)).is_ok());
                    }
                });
            }
        });
        let mut next = [0; 4];
        for event in rx.poll() {
            if let NetEvent::Packet(conn, _, seq) = event {
                assert_eq!(seq, next[conn as usize]);
                next[conn as usize] += 1;
            }
        }
        assert_eq!(next, [1_000; 4]);
        assert!(rx.poll().is_empty());
    }

    #[test]
    fn consuming_while_producers_retry_full_credit_does_not_lose_packets() {
        let (tx, rx) = channel();
        std::thread::scope(|s| {
            for conn in 0..4 {
                let tx = tx.clone();
                s.spawn(move || {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                    for seq in 0..10_000 {
                        let mut event = packet(conn, seq);
                        loop {
                            match tx.send(event) {
                                Ok(()) => break,
                                Err(mpsc::TrySendError::Full(returned)) => event = returned,
                                Err(mpsc::TrySendError::Disconnected(_)) => {
                                    panic!("consumer must remain alive")
                                }
                            }
                            assert!(
                                std::time::Instant::now() < deadline,
                                "credit did not return"
                            );
                            std::thread::yield_now();
                        }
                    }
                });
            }
            let mut next = [0; 4];
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while next != [10_000; 4] {
                for event in rx.poll() {
                    if let NetEvent::Packet(conn, _, seq) = event {
                        assert_eq!(seq, next[conn as usize]);
                        next[conn as usize] += 1;
                    }
                }
                assert!(std::time::Instant::now() < deadline, "packets were lost");
                std::thread::yield_now();
            }
        });
        let stats = tx.stats();
        assert!(matches!(
            stats,
            Some(IngressStats {
                queued_packets: 0,
                queued_body_bytes: 0,
                ..
            })
        ));
    }

    #[test]
    fn lan_uses_admission_and_leaves_work_for_the_next_poll() -> std::io::Result<()> {
        let lan = crate::lan::Lan::start_with(
            Arc::new(crate::lan::data::SyncData::empty()),
            crate::lan::NetConfig {
                listen: false,
                announce: false,
                ..crate::lan::NetConfig::loopback(0, String::new(), 8)
            },
        )?;
        for _ in 0..EVENTS_PER_POLL + 1 {
            assert!(lan.shared.events.send(packet(1, 0)).is_ok());
        }
        assert_eq!(lan.poll().len(), EVENTS_PER_POLL);
        assert_eq!(lan.poll().len(), 1);
        assert!(lan.poll().is_empty());
        assert!(matches!(
            lan.shared.events.send(packet(1, usize::MAX)),
            Err(mpsc::TrySendError::Full(_))
        ));
        Ok(())
    }
}
