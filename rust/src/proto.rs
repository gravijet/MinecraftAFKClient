//! Paket-IDs für **Minecraft 26.1 (Protokoll 775)**.
//!
//! Die IDs sind nicht geraten: sie entsprechen der Registrierungsreihenfolge im Codec von
//! MCProtocolLib 26.1-1 (`MinecraftCodec.CODEC`), die pro Zustand und Richtung bei 0 beginnt –
//! also exakt dieselbe Quelle, aus der auch der Java-Client seine IDs bezieht.

pub const PROTOCOL_VERSION: i32 = 775;
pub const MINECRAFT_VERSION: &str = "26.1";

/// Protokollzustand. Die Zahlenwerte sind nur intern.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Login,
    Configuration,
    Game,
}

/// Handshake (nur ein Paket).
pub mod handshake {
    pub const SB_INTENTION: i32 = 0;
    /// intent-Feld: 1=Status, 2=Login, 3=Transfer
    pub const INTENT_LOGIN: i32 = 2;
}

pub mod login {
    // Server -> Client
    pub const CB_DISCONNECT: i32 = 0;
    pub const CB_HELLO: i32 = 1;
    pub const CB_FINISHED: i32 = 2;
    pub const CB_COMPRESSION: i32 = 3;
    pub const CB_CUSTOM_QUERY: i32 = 4;
    pub const CB_COOKIE_REQUEST: i32 = 5;

    // Client -> Server
    pub const SB_HELLO: i32 = 0;
    pub const SB_KEY: i32 = 1;
    pub const SB_CUSTOM_QUERY_ANSWER: i32 = 2;
    pub const SB_ACKNOWLEDGED: i32 = 3;
    pub const SB_COOKIE_RESPONSE: i32 = 4;
}

pub mod config {
    // Server -> Client
    pub const CB_COOKIE_REQUEST: i32 = 0;
    pub const CB_DISCONNECT: i32 = 2;
    pub const CB_FINISH: i32 = 3;
    pub const CB_KEEP_ALIVE: i32 = 4;
    pub const CB_PING: i32 = 5;
    pub const CB_RESOURCE_PACK_PUSH: i32 = 9;
    pub const CB_STORE_COOKIE: i32 = 10;
    pub const CB_TRANSFER: i32 = 11;
    pub const CB_SELECT_KNOWN_PACKS: i32 = 14;
    pub const CB_CODE_OF_CONDUCT: i32 = 19;

    // Client -> Server
    pub const SB_CLIENT_INFORMATION: i32 = 0;
    pub const SB_COOKIE_RESPONSE: i32 = 1;
    pub const SB_FINISH: i32 = 3;
    pub const SB_KEEP_ALIVE: i32 = 4;
    pub const SB_PONG: i32 = 5;
    pub const SB_RESOURCE_PACK: i32 = 6;
    pub const SB_SELECT_KNOWN_PACKS: i32 = 7;
    pub const SB_ACCEPT_CODE_OF_CONDUCT: i32 = 9;
}

pub mod game {
    // Server -> Client
    pub const CB_COOKIE_REQUEST: i32 = 21;
    pub const CB_DISCONNECT: i32 = 32;
    pub const CB_KEEP_ALIVE: i32 = 44;
    pub const CB_LOGIN: i32 = 49;
    pub const CB_PING: i32 = 61;
    pub const CB_PLAYER_CHAT: i32 = 65;
    pub const CB_PLAYER_POSITION: i32 = 72;
    pub const CB_RESOURCE_PACK_PUSH: i32 = 81;
    pub const CB_SET_HEALTH: i32 = 104;
    pub const CB_START_CONFIGURATION: i32 = 118;
    pub const CB_STORE_COOKIE: i32 = 120;
    pub const CB_SYSTEM_CHAT: i32 = 121;
    pub const CB_TRANSFER: i32 = 129;

    // Client -> Server
    pub const SB_ACCEPT_TELEPORTATION: i32 = 0;
    pub const SB_CLIENT_INFORMATION: i32 = 14;
    pub const SB_CHAT_ACK: i32 = 6;
    pub const SB_CHAT_COMMAND: i32 = 7;
    pub const SB_CHAT: i32 = 9;
    pub const SB_CLIENT_COMMAND: i32 = 12;
    pub const SB_CONFIGURATION_ACKNOWLEDGED: i32 = 16;
    pub const SB_COOKIE_RESPONSE: i32 = 21;
    pub const SB_KEEP_ALIVE: i32 = 28;
    pub const SB_MOVE_PLAYER_POS_ROT: i32 = 31;
    pub const SB_PONG: i32 = 45;
    pub const SB_RESOURCE_PACK: i32 = 49;

    /// ClientCommand: 0 = Respawn (versionsübergreifend stabil).
    pub const CLIENT_COMMAND_RESPAWN: i32 = 0;
}

/// ResourcePackStatus-Ordinalwerte (siehe ResourcePackStatus in MCProtocolLib).
pub mod pack_status {
    pub const SUCCESSFULLY_LOADED: i32 = 0;
    pub const ACCEPTED: i32 = 3;
}
