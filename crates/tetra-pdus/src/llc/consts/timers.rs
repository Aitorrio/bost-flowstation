use tetra_core::{frames, multiframes};

// Timers as defined in Annex A.1 LLC timers
pub const T251_SENDER_RETRY_TIMER: u32 = frames!(4); // 4 signalling frames

/// Non-ETSI, implementation choice. After the ETSI N.252 retransmit budget is spent (~0.9 s on a
/// single carrier), keep the acknowledged-mode entry alive — without any further airtime — for
/// this long to still accept a late BL-ACK before declaring the TL-SDU lost. Energy-economy and
/// some older terminals (MTH800 / MTP850 / Hytera) routinely ACK a frame or two after that budget
/// closes; discarding that late ACK as "unexpected" left MM's ciphering / registration transactions
/// unconfirmed, forcing constant re-registration churn and dropped group audio. One multiframe
/// (~1.02 s) comfortably covers the late ACKs seen on air; the LLC adds the radio's own monitoring
/// cycle on top for a radio that sleeps (see `Llc::ack_grace_window`).
pub const ACK_GRACE_WINDOW: u32 = multiframes!(1);
pub const T252_ACK_WAITING_TIMER: u32 = frames!(9);
pub const T261_SETUP_WAITING_TIMER: u32 = frames!(4);
pub const T263_DISCONNECT_WAITING_TIMER: u32 = frames!(4);
pub const T265_RECONNECT_WAITING_TIMER: u32 = frames!(4);
pub const T271_RECEIVER_NOT_READY_FOR_TX_TIMER: u32 = frames!(36);
pub const T272_RECEIVER_NOT_READY_FOR_RX_TIMER: u32 = frames!(18);
