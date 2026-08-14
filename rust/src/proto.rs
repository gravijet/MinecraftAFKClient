//! Paket-IDs und Formatweichen der unterstützten Minecraft-Protokolle.
//!
//! Die IDs sind nicht geraten: sie entsprechen der Registrierungsreihenfolge im Codec von
//! MCProtocolLib (`MinecraftCodec.CODEC`), die pro Zustand und Richtung bei 0 beginnt – also
//! genau der Quelle, aus der auch der Java-Client seine IDs bezieht. Abgelesen aus den Jars
//! `protocol-1.21` (767), `protocol-1.21.11-1` (774), `protocol-26.1-1` (775) und
//! `protocol-26.2` (776).
//!
//! Bei einem MC-Update dort neu ablesen, nicht raten.

/// Protokollzustand. Die Zahlenwerte sind nur intern.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Login,
    Configuration,
    Game,
}

/// Ein unterstütztes Protokoll: Name für die Kommandozeile, Versionsnummer für den Handshake,
/// die Paket-IDs der Spielphase und die Formatweiche [`Protocol::modern`].
pub struct Protocol {
    /// So heißt die Version auf der Kommandozeile (`--mc 26.1`).
    pub name: &'static str,
    /// Protokollnummer im Handshake.
    pub version: i32,
    /// Paketformate ab 1.21.2 statt 1.21/1.21.1. Betrifft genau vier Stellen, alle geprüft:
    /// Prüfsumme im Chat-Paket, `globalIndex` im Spieler-Chat, Partikel-Status in den
    /// Client-Einstellungen und das Positionspaket (Vektor- statt Einzelfeldformat).
    /// Zusätzlich gibt es den Verhaltenskodex der Konfigurationsphase nur hier.
    pub modern: bool,
    /// IDs der Spielphase – die einzigen, die sich zwischen den Versionen verschieben.
    pub game: Game,
}

/// Paket-IDs der Spielphase.
pub struct Game {
    // Server -> Client
    pub cb_cookie_request: i32,
    pub cb_disconnect: i32,
    pub cb_keep_alive: i32,
    pub cb_login: i32,
    pub cb_ping: i32,
    pub cb_player_chat: i32,
    pub cb_player_position: i32,
    pub cb_resource_pack_push: i32,
    pub cb_set_health: i32,
    pub cb_start_configuration: i32,
    pub cb_store_cookie: i32,
    pub cb_system_chat: i32,
    pub cb_transfer: i32,

    // Client -> Server
    pub sb_accept_teleportation: i32,
    pub sb_chat: i32,
    pub sb_chat_ack: i32,
    pub sb_chat_command: i32,
    pub sb_client_command: i32,
    pub sb_client_information: i32,
    pub sb_configuration_acknowledged: i32,
    pub sb_cookie_response: i32,
    pub sb_keep_alive: i32,
    pub sb_move_player_pos_rot: i32,
    pub sb_pong: i32,
    pub sb_resource_pack: i32,
}

/// Was ein Paket der Spielphase für uns bedeutet. Alles, was hier nicht auftaucht (Chunks,
/// Entitäten, Inventar ...), wird ungelesen verworfen – das ist der halbe Ressourcenvorteil.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum In {
    KeepAlive,
    Ping,
    Login,
    SystemChat,
    PlayerChat,
    Position,
    SetHealth,
    ResourcePackPush,
    StartConfiguration,
    StoreCookie,
    CookieRequest,
    Transfer,
    Disconnect,
    Ignored,
}

pub const PROTOCOLS: &[Protocol] = &[P1_21_1, P1_21_11, P26_1, P26_2];

/// Ohne `--mc` gilt die neueste stabile Version.
pub const DEFAULT: &Protocol = &P26_1;

impl Protocol {
    /// Protokoll zum Namen von der Kommandozeile; `None`, wenn wir es nicht sprechen.
    pub fn find(name: &str) -> Option<&'static Protocol> {
        let name = name.trim();
        PROTOCOLS.iter().find(|p| p.name == name)
    }

    /// Alle Namen für Hilfe und Fehlermeldungen: `1.21.1 | 1.21.11 | 26.1 | 26.2`.
    pub fn names() -> String {
        PROTOCOLS
            .iter()
            .map(|p| p.name)
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

impl Game {
    /// Paket-ID einordnen. Reihenfolge nach Häufigkeit: KeepAlive und Chat kommen ständig,
    /// ein Beitritt genau einmal.
    pub fn incoming(&self, id: i32) -> In {
        if id == self.cb_keep_alive {
            In::KeepAlive
        } else if id == self.cb_system_chat {
            In::SystemChat
        } else if id == self.cb_player_chat {
            In::PlayerChat
        } else if id == self.cb_player_position {
            In::Position
        } else if id == self.cb_ping {
            In::Ping
        } else if id == self.cb_set_health {
            In::SetHealth
        } else if id == self.cb_login {
            In::Login
        } else if id == self.cb_disconnect {
            In::Disconnect
        } else if id == self.cb_transfer {
            In::Transfer
        } else if id == self.cb_resource_pack_push {
            In::ResourcePackPush
        } else if id == self.cb_start_configuration {
            In::StartConfiguration
        } else if id == self.cb_store_cookie {
            In::StoreCookie
        } else if id == self.cb_cookie_request {
            In::CookieRequest
        } else {
            In::Ignored
        }
    }
}

// ===================== Tabellen =====================

const P1_21_1: Protocol = Protocol {
    name: "1.21.1",
    version: 767,
    modern: false,
    game: Game {
        cb_cookie_request: 22,
        cb_disconnect: 29,
        cb_keep_alive: 38,
        cb_login: 43,
        cb_ping: 53,
        cb_player_chat: 57,
        cb_player_position: 64,
        cb_resource_pack_push: 70,
        cb_set_health: 93,
        cb_start_configuration: 105,
        cb_store_cookie: 107,
        cb_system_chat: 108,
        cb_transfer: 115,

        sb_accept_teleportation: 0,
        sb_chat: 6,
        sb_chat_ack: 3,
        sb_chat_command: 4,
        sb_client_command: 9,
        sb_client_information: 10,
        sb_configuration_acknowledged: 12,
        sb_cookie_response: 17,
        sb_keep_alive: 24,
        sb_move_player_pos_rot: 27,
        sb_pong: 39,
        sb_resource_pack: 43,
    },
};

const P1_21_11: Protocol = Protocol {
    name: "1.21.11",
    version: 774,
    modern: true,
    game: Game {
        cb_cookie_request: 21,
        cb_disconnect: 32,
        cb_keep_alive: 43,
        cb_login: 48,
        cb_ping: 59,
        cb_player_chat: 63,
        cb_player_position: 70,
        cb_resource_pack_push: 79,
        cb_set_health: 102,
        cb_start_configuration: 116,
        cb_store_cookie: 118,
        cb_system_chat: 119,
        cb_transfer: 127,

        sb_accept_teleportation: 0,
        sb_chat: 8,
        sb_chat_ack: 5,
        sb_chat_command: 6,
        sb_client_command: 11,
        sb_client_information: 13,
        sb_configuration_acknowledged: 15,
        sb_cookie_response: 20,
        sb_keep_alive: 27,
        sb_move_player_pos_rot: 30,
        sb_pong: 44,
        sb_resource_pack: 48,
    },
};

const P26_1: Protocol = Protocol {
    name: "26.1",
    version: 775,
    modern: true,
    game: GAME_26,
};

/// 26.2 verschiebt keine der von uns benutzten IDs gegenüber 26.1 – nur die Protokollnummer
/// steigt. Nachgeprüft im Codec von `protocol-26.2`; bei einem Update erneut vergleichen.
const P26_2: Protocol = Protocol {
    name: "26.2",
    version: 776,
    modern: true,
    game: GAME_26,
};

const GAME_26: Game = Game {
    cb_cookie_request: 21,
    cb_disconnect: 32,
    cb_keep_alive: 44,
    cb_login: 49,
    cb_ping: 61,
    cb_player_chat: 65,
    cb_player_position: 72,
    cb_resource_pack_push: 81,
    cb_set_health: 104,
    cb_start_configuration: 118,
    cb_store_cookie: 120,
    cb_system_chat: 121,
    cb_transfer: 129,

    sb_accept_teleportation: 0,
    sb_chat: 9,
    sb_chat_ack: 6,
    sb_chat_command: 7,
    sb_client_command: 12,
    sb_client_information: 14,
    sb_configuration_acknowledged: 16,
    sb_cookie_response: 21,
    sb_keep_alive: 28,
    sb_move_player_pos_rot: 31,
    sb_pong: 45,
    sb_resource_pack: 49,
};

// ===================== versionsunabhängige IDs =====================
//
// Handshake, Login und Konfiguration sind in allen vier Protokollen deckungsgleich – bis auf
// den Verhaltenskodex, den es erst ab 1.21.11 gibt (in 1.21.1 hat die Konfigurationsphase gar
// so viele Pakete nicht, die IDs können dort also nicht kollidieren).

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
    /// Erst ab 1.21.11.
    pub const CB_CODE_OF_CONDUCT: i32 = 19;

    // Client -> Server
    pub const SB_CLIENT_INFORMATION: i32 = 0;
    pub const SB_COOKIE_RESPONSE: i32 = 1;
    pub const SB_FINISH: i32 = 3;
    pub const SB_KEEP_ALIVE: i32 = 4;
    pub const SB_PONG: i32 = 5;
    pub const SB_RESOURCE_PACK: i32 = 6;
    pub const SB_SELECT_KNOWN_PACKS: i32 = 7;
    /// Erst ab 1.21.11.
    pub const SB_ACCEPT_CODE_OF_CONDUCT: i32 = 9;
}

/// ClientCommand: 0 = Respawn (versionsübergreifend stabil).
pub const CLIENT_COMMAND_RESPAWN: i32 = 0;

/// ResourcePackStatus-Ordinalwerte (siehe ResourcePackStatus in MCProtocolLib).
pub mod pack_status {
    pub const SUCCESSFULLY_LOADED: i32 = 0;
    pub const ACCEPTED: i32 = 3;
}
