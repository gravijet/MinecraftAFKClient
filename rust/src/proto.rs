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
    /// Zusätzliche IDs, die nur der Premium-Client braucht (Anzeigetafel, Tab-Liste, Menüs,
    /// Schleichen). Im schlanken Build ist davon kein Byte einkompiliert.
    #[cfg(feature = "premium")]
    pub extra: Extra,
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
    pub cb_respawn: i32,
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

/// Paket-IDs, die nur der Premium-Build braucht. Ausgelagert, damit der schlanke Client
/// unverändert bleibt: ohne `--features premium` gibt es weder die Tabelle noch den Code, der
/// sie liest.
#[cfg(feature = "premium")]
pub struct Extra {
    // Server -> Client
    pub cb_container_close: i32,
    pub cb_container_set_content: i32,
    pub cb_container_set_slot: i32,
    pub cb_open_screen: i32,
    pub cb_player_info_remove: i32,
    pub cb_player_info_update: i32,
    pub cb_reset_score: i32,
    pub cb_set_display_objective: i32,
    pub cb_set_objective: i32,
    pub cb_set_player_team: i32,
    pub cb_set_score: i32,

    // Client -> Server
    pub sb_container_click: i32,
    pub sb_container_close: i32,
    pub sb_player_command: i32,
    pub sb_player_input: i32,
    pub sb_set_carried_item: i32,
    pub sb_swing: i32,
    pub sb_use_item: i32,

    /// Anzahl der Werte von `PlayerListEntryAction`: so breit ist das Bitfeld vorn im
    /// Tab-Listen-Paket (1.21.1 kennt sechs, ab 1.21.11 acht). Beides passt in ein Byte.
    pub player_info_actions: u32,

    /// Feldreihenfolge im Team-Paket – das einzige von uns gelesene Paket, dessen Aufbau sich
    /// zwischen den vier Versionen dreimal ändert.
    pub team_layout: TeamLayout,
}

/// Aufbau von `ClientboundSetPlayerTeamPacket` nach `name` und `action`, für die Aktionen
/// „anlegen" und „ändern". Abgelesen aus den vier Codec-Jars.
#[cfg(feature = "premium")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TeamLayout {
    /// 1.21.1: Anzeigename, Flags (Byte), Sichtbarkeit **als Zeichenkette**, Kollision **als
    /// Zeichenkette**, Farbe (VarInt), Präfix, Suffix.
    Legacy,
    /// 1.21.11 und 26.1: wie [`TeamLayout::Legacy`], aber Sichtbarkeit und Kollision als VarInt.
    VarIntRules,
    /// 26.2: Anzeigename, **Präfix, Suffix**, Sichtbarkeit, Kollision, Farbe (optional), Flags.
    Reordered,
}

/// Was ein Paket der Spielphase für uns bedeutet. Alles, was hier nicht auftaucht (Chunks,
/// Entitäten, Blöcke ...), wird ungelesen verworfen – das ist der halbe Ressourcenvorteil.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum In {
    KeepAlive,
    Ping,
    Login,
    SystemChat,
    PlayerChat,
    Position,
    Respawn,
    SetHealth,
    ResourcePackPush,
    StartConfiguration,
    StoreCookie,
    CookieRequest,
    Transfer,
    Disconnect,

    // ---- nur im Premium-Build ----
    /// Anzeigetafel: Ziel angelegt/geändert/entfernt.
    #[cfg(feature = "premium")]
    Objective,
    /// Anzeigetafel: Punktzahl gesetzt.
    #[cfg(feature = "premium")]
    Score,
    /// Anzeigetafel: Punktzahl entfernt.
    #[cfg(feature = "premium")]
    ResetScore,
    /// Anzeigetafel: welches Ziel in welchem Bereich (Seitenleiste, Tab-Liste ...) steht.
    #[cfg(feature = "premium")]
    DisplayObjective,
    /// Team (liefert Präfix/Suffix der Namen in der Seitenleiste).
    #[cfg(feature = "premium")]
    Team,
    #[cfg(feature = "premium")]
    PlayerInfoUpdate,
    #[cfg(feature = "premium")]
    PlayerInfoRemove,
    #[cfg(feature = "premium")]
    OpenScreen,
    #[cfg(feature = "premium")]
    ContainerContent,
    #[cfg(feature = "premium")]
    ContainerSlot,
    #[cfg(feature = "premium")]
    ContainerClose,

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

impl Protocol {
    /// Paket-ID einordnen. Reihenfolge nach Häufigkeit: KeepAlive und Chat kommen ständig,
    /// ein Beitritt genau einmal. Alles Unbekannte ist [`In::Ignored`] und wird nie gelesen.
    pub fn incoming(&self, id: i32) -> In {
        let g = &self.game;
        if id == g.cb_keep_alive {
            In::KeepAlive
        } else if id == g.cb_system_chat {
            In::SystemChat
        } else if id == g.cb_player_chat {
            In::PlayerChat
        } else if id == g.cb_player_position {
            In::Position
        } else if id == g.cb_ping {
            In::Ping
        } else if id == g.cb_set_health {
            In::SetHealth
        } else if id == g.cb_login {
            In::Login
        } else if id == g.cb_respawn {
            In::Respawn
        } else if id == g.cb_disconnect {
            In::Disconnect
        } else if id == g.cb_transfer {
            In::Transfer
        } else if id == g.cb_resource_pack_push {
            In::ResourcePackPush
        } else if id == g.cb_start_configuration {
            In::StartConfiguration
        } else if id == g.cb_store_cookie {
            In::StoreCookie
        } else if id == g.cb_cookie_request {
            In::CookieRequest
        } else {
            self.incoming_extra(id)
        }
    }

    #[cfg(not(feature = "premium"))]
    fn incoming_extra(&self, _id: i32) -> In {
        In::Ignored
    }

    /// Die Pakete, die nur der Premium-Client auswertet. Bewusst hinter den häufigen: ein
    /// Chunk-Paket läuft zwar durch die ganze Kette, das sind aber ein paar Zahlenvergleiche.
    #[cfg(feature = "premium")]
    fn incoming_extra(&self, id: i32) -> In {
        let e = &self.extra;
        if id == e.cb_set_score {
            In::Score
        } else if id == e.cb_set_objective {
            In::Objective
        } else if id == e.cb_reset_score {
            In::ResetScore
        } else if id == e.cb_set_display_objective {
            In::DisplayObjective
        } else if id == e.cb_set_player_team {
            In::Team
        } else if id == e.cb_player_info_update {
            In::PlayerInfoUpdate
        } else if id == e.cb_player_info_remove {
            In::PlayerInfoRemove
        } else if id == e.cb_open_screen {
            In::OpenScreen
        } else if id == e.cb_container_set_content {
            In::ContainerContent
        } else if id == e.cb_container_set_slot {
            In::ContainerSlot
        } else if id == e.cb_container_close {
            In::ContainerClose
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
    #[cfg(feature = "premium")]
    extra: Extra {
        cb_container_close: 18,
        cb_container_set_content: 19,
        cb_container_set_slot: 21,
        cb_open_screen: 51,
        cb_player_info_remove: 61,
        cb_player_info_update: 62,
        cb_reset_score: 68,
        cb_set_display_objective: 87,
        cb_set_objective: 94,
        cb_set_player_team: 96,
        cb_set_score: 97,

        sb_container_click: 14,
        sb_container_close: 15,
        sb_player_command: 37,
        // 1.21.1 kennt das Eingabepaket nur für Fahrzeuge (zwei Floats + Byte). Geschlichen
        // wird hier über sb_player_command, deshalb steht die ID nur der Vollständigkeit halber.
        sb_player_input: 38,
        sb_set_carried_item: 47,
        sb_swing: 54,
        sb_use_item: 57,

        player_info_actions: 6,
        team_layout: TeamLayout::Legacy,
    },
    game: Game {
        cb_cookie_request: 22,
        cb_disconnect: 29,
        cb_keep_alive: 38,
        cb_login: 43,
        cb_ping: 53,
        cb_player_chat: 57,
        cb_player_position: 64,
        cb_resource_pack_push: 70,
        cb_respawn: 71,
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
    #[cfg(feature = "premium")]
    extra: Extra {
        cb_container_close: 17,
        cb_container_set_content: 18,
        cb_container_set_slot: 20,
        cb_open_screen: 57,
        cb_player_info_remove: 67,
        cb_player_info_update: 68,
        cb_reset_score: 77,
        cb_set_display_objective: 96,
        cb_set_objective: 104,
        cb_set_player_team: 107,
        cb_set_score: 108,

        sb_container_click: 17,
        sb_container_close: 18,
        sb_player_command: 41,
        sb_player_input: 42,
        sb_set_carried_item: 52,
        sb_swing: 60,
        sb_use_item: 64,

        player_info_actions: 8,
        team_layout: TeamLayout::VarIntRules,
    },
    game: Game {
        cb_cookie_request: 21,
        cb_disconnect: 32,
        cb_keep_alive: 43,
        cb_login: 48,
        cb_ping: 59,
        cb_player_chat: 63,
        cb_player_position: 70,
        cb_resource_pack_push: 79,
        cb_respawn: 80,
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
    #[cfg(feature = "premium")]
    extra: Extra {
        team_layout: TeamLayout::VarIntRules,
        ..EXTRA_26
    },
    game: GAME_26,
};

/// 26.2 verschiebt keine der von uns benutzten IDs gegenüber 26.1 – nur die Protokollnummer
/// steigt. Ein Paketaufbau ändert sich aber doch: das Team-Paket, siehe [`TeamLayout`].
/// Nachgeprüft im Codec von `protocol-26.2`; bei einem Update erneut vergleichen.
const P26_2: Protocol = Protocol {
    name: "26.2",
    version: 776,
    modern: true,
    #[cfg(feature = "premium")]
    extra: Extra {
        team_layout: TeamLayout::Reordered,
        ..EXTRA_26
    },
    game: GAME_26,
};

#[cfg(feature = "premium")]
const EXTRA_26: Extra = Extra {
    cb_container_close: 17,
    cb_container_set_content: 18,
    cb_container_set_slot: 20,
    cb_open_screen: 59,
    cb_player_info_remove: 69,
    cb_player_info_update: 70,
    cb_reset_score: 79,
    cb_set_display_objective: 98,
    cb_set_objective: 106,
    cb_set_player_team: 109,
    cb_set_score: 110,

    sb_container_click: 18,
    sb_container_close: 19,
    sb_player_command: 42,
    sb_player_input: 43,
    sb_set_carried_item: 53,
    sb_swing: 63,
    sb_use_item: 67,

    player_info_actions: 8,
    // Wird in beiden Protokollen gesetzt – 26.1 und 26.2 unterscheiden sich genau hier.
    team_layout: TeamLayout::VarIntRules,
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
    cb_respawn: 82,
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

/// Zahlenwerte in den Zusatzpaketen des Premium-Clients. Alle aus denselben Klassen abgelesen
/// wie die Paket-IDs (Aufzählungsreihenfolge = übertragener Wert).
#[cfg(feature = "premium")]
pub mod values {
    /// `PlayerState` in 1.21.1 – dort werden Schleichen und Sprinten noch als Spielerbefehl
    /// geschickt. Ab 1.21.11 gibt es diese vier Werte nicht mehr; dort trägt das Eingabepaket
    /// die Zustände (siehe [`input`]).
    pub mod player_state {
        pub const START_SNEAKING: i32 = 0;
        pub const STOP_SNEAKING: i32 = 1;
        pub const START_SPRINTING: i32 = 3;
        pub const STOP_SPRINTING: i32 = 4;
    }

    /// Bits im Eingabepaket ab 1.21.11 (`ServerboundPlayerInputPacket`, ein Byte).
    pub mod input {
        pub const SNEAK: u8 = 0x20;
        pub const SPRINT: u8 = 0x40;
    }

    /// `ContainerActionType` – der Modus im Klick-Paket.
    pub mod click {
        /// Normaler Klick (Knopf 0 = links, 1 = rechts).
        pub const NORMAL: u8 = 0;
        /// Umschalt-Klick (Knopf 0).
        pub const SHIFT: u8 = 1;
    }

    /// `ScoreboardPosition` – uns interessiert nur die Seitenleiste.
    pub const SIDEBAR: i32 = 1;

    /// `ObjectiveAction`
    pub mod objective {
        pub const ADD: u8 = 0;
        pub const REMOVE: u8 = 1;
        pub const UPDATE: u8 = 2;
    }

    /// `TeamAction`
    pub mod team {
        pub const CREATE: u8 = 0;
        pub const REMOVE: u8 = 1;
        pub const UPDATE: u8 = 2;
        pub const ADD_PLAYER: u8 = 3;
        pub const REMOVE_PLAYER: u8 = 4;
    }

    /// Bits im Bitfeld von `ClientboundPlayerInfoUpdatePacket` (Reihenfolge von
    /// `PlayerListEntryAction`). Die letzten beiden gibt es erst ab 1.21.11.
    pub mod info {
        pub const ADD_PLAYER: u32 = 1 << 0;
        pub const INITIALIZE_CHAT: u32 = 1 << 1;
        pub const UPDATE_GAME_MODE: u32 = 1 << 2;
        pub const UPDATE_LISTED: u32 = 1 << 3;
        pub const UPDATE_LATENCY: u32 = 1 << 4;
        pub const UPDATE_DISPLAY_NAME: u32 = 1 << 5;
        pub const UPDATE_LIST_ORDER: u32 = 1 << 6;
        pub const UPDATE_HAT: u32 = 1 << 7;
    }
}
