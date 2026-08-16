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
    /// Paketformate ab 1.21.2 statt 1.21/1.21.1. Betrifft geprüfte Stellen: Prüfsumme im
    /// Chat-Paket, `globalIndex` im Spieler-Chat, Partikel-Status in den Client-Einstellungen,
    /// das Positionspaket (Vektor- statt Einzelfeldformat), den Verhaltenskodex der
    /// Konfigurationsphase sowie – nur in den Ausbaustufen – das Chunk-Format (Höhenkarten als
    /// Liste statt NBT, Palettendaten ohne Längenangabe) und `ClientboundTeleportEntity`.
    pub modern: bool,
    /// IDs der Spielphase – die einzigen, die sich zwischen den Versionen verschieben.
    pub game: Game,
    /// Zusätzliche IDs der Ausbaustufen (Anzeigetafel, Menüs, Tastenzustand, Live-Ansicht).
    /// Im schlanken Build ist davon kein Byte einkompiliert.
    #[cfg(feature = "extras")]
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

/// Paket-IDs und Formatangaben, die nur die Ausbaustufen brauchen. Ausgelagert, damit der
/// schlanke Client unverändert bleibt: ohne eine Zusatzfunktion gibt es weder die Tabelle noch
/// den Code, der sie liest.
///
/// Die Tabelle ist für alle Ausbaustufen dieselbe (ein paar hundert Byte statischer Daten); der
/// **Code** dazu steckt dagegen je Funktion in einem eigenen Cargo-Feature. Ein Build ohne
/// Live-Ansicht liest also nie ein Chunk-Paket, auch wenn dessen ID in der Tabelle steht.
#[cfg(feature = "extras")]
#[allow(dead_code)] // je nach Bauform bleibt ein Teil der Tabelle ungenutzt
pub struct Extra {
    // ---- Anzeigetafel ----
    pub cb_reset_score: i32,
    pub cb_set_display_objective: i32,
    pub cb_set_objective: i32,
    pub cb_set_player_team: i32,
    pub cb_set_score: i32,

    // ---- Menüs/Behälter ----
    pub cb_container_close: i32,
    pub cb_container_set_content: i32,
    pub cb_container_set_slot: i32,
    pub cb_open_screen: i32,
    /// Einzelnes Feld des eigenen Inventars (erst ab 1.21.11; in 1.21.1 -1).
    pub cb_set_player_inventory: i32,
    pub sb_container_click: i32,
    pub sb_container_close: i32,

    // ---- Tastenzustand ----
    pub sb_player_command: i32,
    pub sb_player_input: i32,
    pub sb_set_carried_item: i32,
    pub sb_swing: i32,
    pub sb_use_item: i32,

    // ---- Live-Ansicht: Welt ----
    pub cb_level_chunk: i32,
    pub cb_forget_level_chunk: i32,
    pub cb_block_update: i32,
    pub cb_section_blocks_update: i32,
    /// Nach jedem Chunk-Stapel erwartet der Server eine Bestätigung; ohne sie hört er nach ein
    /// paar Stapeln auf, weitere Chunks zu schicken.
    pub cb_chunk_batch_finished: i32,
    pub sb_chunk_batch_received: i32,

    // ---- Live-Ansicht: Entitäten ----
    pub cb_add_entity: i32,
    pub cb_remove_entities: i32,
    pub cb_move_entity_pos: i32,
    pub cb_move_entity_pos_rot: i32,
    pub cb_teleport_entity: i32,
    /// Erst ab 1.21.2; in 1.21.1 gibt es das Paket nicht (dann -1, passt auf keine ID).
    pub cb_entity_position_sync: i32,
    /// Registrierungsnummer des Entitätstyps `minecraft:player` – damit unterscheidet die
    /// Live-Ansicht Spieler von allem anderen. Abgelesen aus der Reihenfolge von `EntityType`
    /// (dort ist `from(id)` schlicht `VALUES[id]`).
    pub player_entity_type: i32,

    /// Ab 26.1 steht in jedem Chunk-Abschnitt vor den Paletten **zweimal** ein Short
    /// (Block- und Flüssigkeitszähler), davor nur einmal.
    pub section_fluid_count: bool,

    /// Feldreihenfolge im Team-Paket – das einzige von uns gelesene Paket, dessen Aufbau sich
    /// zwischen den vier Versionen dreimal ändert.
    pub team_layout: TeamLayout,

    /// Gegenstands-Komponenten (nur mit `--features items`).
    #[cfg(feature = "items")]
    pub components: Components,
}

/// Aufbau von `ClientboundSetPlayerTeamPacket` nach `name` und `action`, für die Aktionen
/// „anlegen" und „ändern". Abgelesen aus den vier Codec-Jars.
#[cfg(feature = "board")]
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

/// Ohne Anzeigetafel wird das Team-Paket nie gelesen; das Feld bleibt trotzdem in der Tabelle,
/// damit die vier Versionstabellen in jeder Bauform gleich aussehen.
#[cfg(all(feature = "extras", not(feature = "board")))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TeamLayout {
    Legacy,
    VarIntRules,
    Reordered,
}

/// Wo die Namen und die Lore eines Gegenstands stehen und wie sich die übrigen Komponenten
/// überspringen lassen. Siehe [`crate::items`] – dort steht auch, was die Buchstaben bedeuten.
#[cfg(feature = "items")]
pub struct Components {
    /// Netz-ID von `minecraft:custom_name` (der vom Server gesetzte Anzeigename).
    pub custom_name: u8,
    /// Netz-ID von `minecraft:item_name` (Name des Gegenstands selbst, falls kein Anzeigename).
    pub item_name: u8,
    /// Netz-ID von `minecraft:lore`.
    pub lore: u8,
    /// Form je Komponenten-ID (Index = ID). Ein Punkt heißt „Aufbau nicht hinterlegt".
    pub shapes: &'static str,
}

/// Was ein Paket der Spielphase für uns bedeutet. Alles, was hier nicht auftaucht, wird ungelesen
/// verworfen – das ist der halbe Ressourcenvorteil.
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

    // ---- Anzeigetafel ----
    /// Ziel angelegt/geändert/entfernt.
    #[cfg(feature = "board")]
    Objective,
    /// Punktzahl gesetzt.
    #[cfg(feature = "board")]
    Score,
    /// Punktzahl entfernt.
    #[cfg(feature = "board")]
    ResetScore,
    /// Welches Ziel in welchem Bereich (Seitenleiste ...) steht.
    #[cfg(feature = "board")]
    DisplayObjective,
    /// Team (liefert Präfix/Suffix der Namen in der Seitenleiste).
    #[cfg(feature = "board")]
    Team,

    // ---- Menüs ----
    #[cfg(feature = "menu")]
    OpenScreen,
    #[cfg(feature = "menu")]
    ContainerContent,
    #[cfg(feature = "menu")]
    ContainerSlot,
    #[cfg(feature = "menu")]
    ContainerClose,
    /// Einzelnes Feld des eigenen Inventars (nur wenn Feldinhalte gelesen werden).
    #[cfg(feature = "items")]
    PlayerInventory,

    // ---- Live-Ansicht ----
    #[cfg(feature = "pov")]
    LevelChunk,
    #[cfg(feature = "pov")]
    ForgetChunk,
    #[cfg(feature = "pov")]
    BlockUpdate,
    #[cfg(feature = "pov")]
    SectionBlocks,
    #[cfg(feature = "pov")]
    ChunkBatchFinished,
    #[cfg(feature = "pov")]
    AddEntity,
    #[cfg(feature = "pov")]
    RemoveEntities,
    #[cfg(feature = "pov")]
    MoveEntityPos,
    #[cfg(feature = "pov")]
    MoveEntityPosRot,
    #[cfg(feature = "pov")]
    TeleportEntity,
    #[cfg(feature = "pov")]
    EntityPositionSync,

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

    #[cfg(not(feature = "extras"))]
    fn incoming_extra(&self, _id: i32) -> In {
        In::Ignored
    }

    /// Die Pakete, die nur eine Ausbaustufe auswertet. Bewusst hinter den häufigen des schlanken
    /// Clients: ein Chunk-Paket läuft zwar durch die ganze Kette, das sind aber ein paar
    /// Zahlenvergleiche.
    #[cfg(feature = "extras")]
    fn incoming_extra(&self, id: i32) -> In {
        #[allow(unused_variables)]
        let e = &self.extra;

        // Welt- und Entitätspakete kommen im Sekundentakt – die stehen deshalb vorn.
        #[cfg(feature = "pov")]
        {
            if id == e.cb_level_chunk {
                return In::LevelChunk;
            } else if id == e.cb_block_update {
                return In::BlockUpdate;
            } else if id == e.cb_move_entity_pos {
                return In::MoveEntityPos;
            } else if id == e.cb_move_entity_pos_rot {
                return In::MoveEntityPosRot;
            } else if id == e.cb_teleport_entity {
                return In::TeleportEntity;
            } else if id == e.cb_entity_position_sync {
                return In::EntityPositionSync;
            } else if id == e.cb_add_entity {
                return In::AddEntity;
            } else if id == e.cb_remove_entities {
                return In::RemoveEntities;
            } else if id == e.cb_section_blocks_update {
                return In::SectionBlocks;
            } else if id == e.cb_forget_level_chunk {
                return In::ForgetChunk;
            } else if id == e.cb_chunk_batch_finished {
                return In::ChunkBatchFinished;
            }
        }

        #[cfg(feature = "board")]
        {
            if id == e.cb_set_score {
                return In::Score;
            } else if id == e.cb_set_objective {
                return In::Objective;
            } else if id == e.cb_reset_score {
                return In::ResetScore;
            } else if id == e.cb_set_display_objective {
                return In::DisplayObjective;
            } else if id == e.cb_set_player_team {
                return In::Team;
            }
        }

        #[cfg(feature = "menu")]
        {
            if id == e.cb_open_screen {
                return In::OpenScreen;
            } else if id == e.cb_container_set_content {
                return In::ContainerContent;
            } else if id == e.cb_container_set_slot {
                return In::ContainerSlot;
            } else if id == e.cb_container_close {
                return In::ContainerClose;
            }
            #[cfg(feature = "items")]
            if id == e.cb_set_player_inventory {
                return In::PlayerInventory;
            }
        }

        In::Ignored
    }
}

// ===================== Tabellen =====================

const P1_21_1: Protocol = Protocol {
    name: "1.21.1",
    version: 767,
    modern: false,
    #[cfg(feature = "extras")]
    extra: Extra {
        cb_reset_score: 68,
        cb_set_display_objective: 87,
        cb_set_objective: 94,
        cb_set_player_team: 96,
        cb_set_score: 97,

        cb_container_close: 18,
        cb_container_set_content: 19,
        cb_container_set_slot: 21,
        cb_open_screen: 51,
        cb_set_player_inventory: -1, // gibt es erst ab 1.21.11
        sb_container_click: 14,
        sb_container_close: 15,

        sb_player_command: 37,
        // 1.21.1 kennt das Eingabepaket nur für Fahrzeuge (zwei Floats + Byte). Geschlichen
        // wird hier über sb_player_command, deshalb steht die ID nur der Vollständigkeit halber.
        sb_player_input: 38,
        sb_set_carried_item: 47,
        sb_swing: 54,
        sb_use_item: 57,

        cb_level_chunk: 39,
        cb_forget_level_chunk: 33,
        cb_block_update: 9,
        cb_section_blocks_update: 73,
        cb_chunk_batch_finished: 12,
        sb_chunk_batch_received: 8,

        cb_add_entity: 1,
        cb_remove_entities: 66,
        cb_move_entity_pos: 46,
        cb_move_entity_pos_rot: 47,
        cb_teleport_entity: 112,
        cb_entity_position_sync: -1, // gibt es erst ab 1.21.2
        player_entity_type: 128,

        section_fluid_count: false,
        team_layout: TeamLayout::Legacy,
        #[cfg(feature = "items")]
        components: Components {
            custom_name: 5,
            item_name: 6,
            lore: 7,
            shapes: SHAPES_1_21_1,
        },
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
    #[cfg(feature = "extras")]
    extra: Extra {
        cb_reset_score: 77,
        cb_set_display_objective: 96,
        cb_set_objective: 104,
        cb_set_player_team: 107,
        cb_set_score: 108,

        cb_container_close: 17,
        cb_container_set_content: 18,
        cb_container_set_slot: 20,
        cb_open_screen: 57,
        cb_set_player_inventory: 106,
        sb_container_click: 17,
        sb_container_close: 18,

        sb_player_command: 41,
        sb_player_input: 42,
        sb_set_carried_item: 52,
        sb_swing: 60,
        sb_use_item: 64,

        cb_level_chunk: 44,
        cb_forget_level_chunk: 37,
        cb_block_update: 8,
        cb_section_blocks_update: 82,
        cb_chunk_batch_finished: 11,
        sb_chunk_batch_received: 10,

        cb_add_entity: 1,
        cb_remove_entities: 75,
        cb_move_entity_pos: 51,
        cb_move_entity_pos_rot: 52,
        cb_teleport_entity: 123,
        cb_entity_position_sync: 35,
        player_entity_type: 155,

        section_fluid_count: false,
        team_layout: TeamLayout::VarIntRules,
        #[cfg(feature = "items")]
        components: Components {
            custom_name: 6,
            item_name: 9,
            lore: 11,
            shapes: SHAPES_1_21_11,
        },
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
    #[cfg(feature = "extras")]
    extra: Extra {
        team_layout: TeamLayout::VarIntRules,
        player_entity_type: 155,
        #[cfg(feature = "items")]
        components: Components {
            custom_name: 6,
            item_name: 9,
            lore: 11,
            shapes: SHAPES_26_1,
        },
        ..EXTRA_26
    },
    game: GAME_26,
};

/// 26.2 verschiebt keine der von uns benutzten IDs gegenüber 26.1 – nur die Protokollnummer
/// steigt. Zwei Dinge ändern sich doch: das Team-Paket (siehe [`TeamLayout`]) und die
/// Registrierungsnummer des Spieler-Entitätstyps. Nachgeprüft im Codec von `protocol-26.2`;
/// bei einem Update erneut vergleichen.
const P26_2: Protocol = Protocol {
    name: "26.2",
    version: 776,
    modern: true,
    #[cfg(feature = "extras")]
    extra: Extra {
        team_layout: TeamLayout::Reordered,
        player_entity_type: 156,
        #[cfg(feature = "items")]
        components: Components {
            custom_name: 6,
            item_name: 9,
            lore: 11,
            shapes: SHAPES_26_2,
        },
        ..EXTRA_26
    },
    game: GAME_26,
};

#[cfg(feature = "extras")]
const EXTRA_26: Extra = Extra {
    cb_reset_score: 79,
    cb_set_display_objective: 98,
    cb_set_objective: 106,
    cb_set_player_team: 109,
    cb_set_score: 110,

    cb_container_close: 17,
    cb_container_set_content: 18,
    cb_container_set_slot: 20,
    cb_open_screen: 59,
    cb_set_player_inventory: 108,
    sb_container_click: 18,
    sb_container_close: 19,

    sb_player_command: 42,
    sb_player_input: 43,
    sb_set_carried_item: 53,
    sb_swing: 63,
    sb_use_item: 67,

    cb_level_chunk: 45,
    cb_forget_level_chunk: 37,
    cb_block_update: 8,
    cb_section_blocks_update: 84,
    cb_chunk_batch_finished: 11,
    sb_chunk_batch_received: 11,

    cb_add_entity: 1,
    cb_remove_entities: 77,
    cb_move_entity_pos: 53,
    cb_move_entity_pos_rot: 54,
    cb_teleport_entity: 125,
    cb_entity_position_sync: 35,

    section_fluid_count: true,

    // Die folgenden drei Felder setzen 26.1 und 26.2 jeweils selbst – genau hier unterscheiden
    // sie sich. Die Werte stehen trotzdem hier, weil Rust eine vollständige Vorlage verlangt.
    player_entity_type: 155,
    team_layout: TeamLayout::VarIntRules,
    #[cfg(feature = "items")]
    components: Components {
        custom_name: 6,
        item_name: 9,
        lore: 11,
        shapes: SHAPES_26_1,
    },
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

// ===================== Gegenstands-Komponenten =====================
//
// Index = Netz-ID der Komponente (die Registrierungsreihenfolge in `DataComponentTypes`), Wert =
// Aufbau ihrer Daten. Abgelesen aus derselben Jar wie die Paket-IDs: der statische Initialisierer
// nennt je Komponente die Lesefunktion, und die steht hier als ein Buchstabe.
//
//   U nichts    V VarInt   B Bool     F Float    I Int      S Zeichenkette
//   N NBT       C Komponente (NBT)    L Liste von Komponenten (Lore)
//   v Liste von VarInt     H HolderSet   E/e Verzauberungen (mit/ohne Anzeige-Flag)
//   T Anzeigeregeln        M Modelldaten  P/p Spielerprofil (alt/neu)   . nicht hinterlegt
//
// Ein Punkt heißt nicht „geraten", sondern „Aufbau nicht nachgeschlagen": beim Lesen hört der
// Client dort auf (siehe [`crate::items`]).

#[cfg(feature = "items")]
const SHAPES_1_21_1: &str = concat!(
    "NVVV", // 0: custom_data, max_stack_size, max_damage, damage
    "BCCL", // 4: unbreakable, custom_name, item_name, lore
    "VE..", // 8: rarity, enchantments, can_place_on, can_break
    ".VUU", // 12: attribute_modifiers, custom_model_data, hide_additional_tooltip, hide_tooltip
    "VUBN", // 16: repair_cost, creative_slot_lock, enchantment_glint_override, intangible_projectile
    ".U.E", // 20: food, fire_resistant, tool, stored_enchantments
    "..VN", // 24: dyed_color, map_color, map_id, map_decorations
    "V...", // 28: map_post_processing, charged_projectiles, bundle_contents, potion_contents
    "....", // 32: suspicious_stew_effects, writable_book_content, written_book_content, trim
    "NNNN", // 36: debug_stick_state, entity_data, bucket_entity_data, block_entity_data
    ".V..", // 40: instrument, ominous_bottle_amplifier, jukebox_playable, recipes
    "...P", // 44: lodestone_tracker, firework_explosion, fireworks, profile
    "S.Vv", // 48: note_block_sound, banner_patterns, base_color, pot_decorations
    "....", // 52: container, block_state, bees, lock
    "N",    // 56: container_loot
);

#[cfg(feature = "items")]
const SHAPES_1_21_11: &str = concat!(
    "NVVV", // 0: custom_data, max_stack_size, max_damage, damage
    "U.CF", // 4: unbreakable, use_effects, custom_name, minimum_attack_charge
    ".CSL", // 8: damage_type, item_name, item_model, lore
    "Ve..", // 12: rarity, enchantments, can_place_on, can_break
    ".MTV", // 16: attribute_modifiers, custom_model_data, tooltip_display, repair_cost
    "UBN.", // 20: creative_slot_lock, enchantment_glint_override, intangible_projectile, food
    "...S", // 24: consumable, use_remainder, use_cooldown, damage_resistant
    "...V", // 28: tool, weapon, attack_range, enchantable
    ".HUS", // 32: equippable, repairable, glider, tooltip_style
    "....", // 36: death_protection, blocks_attacks, piercing_weapon, kinetic_weapon
    ".eII", // 40: swing_animation, stored_enchantments, dyed_color, map_color
    "VNV.", // 44: map_id, map_decorations, map_post_processing, charged_projectiles
    "..F.", // 48: bundle_contents, potion_contents, potion_duration_scale, suspicious_stew_effects
    "...N", // 52: writable_book_content, written_book_content, trim, debug_stick_state
    ".N..", // 56: entity_data, bucket_entity_data, block_entity_data, instrument
    ".V.S", // 60: provides_trim_material, ominous_bottle_amplifier, jukebox_playable, provides_banner_patterns
    "....", // 64: recipes, lodestone_tracker, firework_explosion, fireworks
    "pS.V", // 68: profile, note_block_sound, banner_patterns, base_color
    "v...", // 72: pot_decorations, container, block_state, bees
    "NN.V", // 76: lock, container_loot, break_sound, villager_variant
    "VVVV", // 80: wolf_variant, wolf_sound_variant, wolf_collar, fox_variant
    "VVVV", // 84: salmon_size, parrot_variant, tropical_fish_pattern, tropical_fish_base_color
    "VVVV", // 88: tropical_fish_pattern_color, mooshroom_variant, rabbit_variant, pig_variant
    "V..V", // 92: cow_variant, chicken_variant, zombie_nautilus_variant, frog_variant
    "V.VV", // 96: horse_variant, painting_variant, llama_variant, axolotl_variant
    "VVVV", // 100: cat_variant, cat_collar, sheep_color, shulker_color
);

#[cfg(feature = "items")]
const SHAPES_26_1: &str = concat!(
    "NVVV", // 0: custom_data, max_stack_size, max_damage, damage
    "U.CF", // 4: unbreakable, use_effects, custom_name, minimum_attack_charge
    "VCSL", // 8: damage_type, item_name, item_model, lore
    "Ve..", // 12: rarity, enchantments, can_place_on, can_break
    ".MTV", // 16: attribute_modifiers, custom_model_data, tooltip_display, repair_cost
    "UBN.", // 20: creative_slot_lock, enchantment_glint_override, intangible_projectile, food
    "...H", // 24: consumable, use_remainder, use_cooldown, damage_resistant
    "...V", // 28: tool, weapon, attack_range, enchantable
    ".HUS", // 32: equippable, repairable, glider, tooltip_style
    "....", // 36: death_protection, blocks_attacks, piercing_weapon, kinetic_weapon
    ".VeV", // 40: swing_animation, additional_trade_cost, stored_enchantments, dye
    "IIVN", // 44: dyed_color, map_color, map_id, map_decorations
    "V...", // 48: map_post_processing, charged_projectiles, bundle_contents, potion_contents
    "F...", // 52: potion_duration_scale, suspicious_stew_effects, writable_book_content, written_book_content
    ".N.N", // 56: trim, debug_stick_state, entity_data, bucket_entity_data
    "...V", // 60: block_entity_data, instrument, provides_trim_material, ominous_bottle_amplifier
    ".H..", // 64: jukebox_playable, provides_banner_patterns, recipes, lodestone_tracker
    "..pS", // 68: firework_explosion, fireworks, profile, note_block_sound
    ".Vv.", // 72: banner_patterns, base_color, pot_decorations, container
    "..NN", // 76: block_state, bees, lock, container_loot
    ".VVV", // 80: break_sound, villager_variant, wolf_variant, wolf_sound_variant
    "VVVV", // 84: wolf_collar, fox_variant, salmon_size, parrot_variant
    "VVVV", // 88: tropical_fish_pattern, tropical_fish_base_color, tropical_fish_pattern_color, mooshroom_variant
    "VVVV", // 92: rabbit_variant, pig_variant, pig_sound_variant, cow_variant
    "VVVV", // 96: cow_sound_variant, chicken_variant, chicken_sound_variant, zombie_nautilus_variant
    "VV.V", // 100: frog_variant, horse_variant, painting_variant, llama_variant
    "VVVV", // 104: axolotl_variant, cat_variant, cat_sound_variant, cat_collar
    "VV",   // 108: sheep_color, shulker_color
);

#[cfg(feature = "items")]
const SHAPES_26_2: &str = concat!(
    "NVVV", // 0: custom_data, max_stack_size, max_damage, damage
    "U.CF", // 4: unbreakable, use_effects, custom_name, minimum_attack_charge
    "VCSL", // 8: damage_type, item_name, item_model, lore
    "Ve..", // 12: rarity, enchantments, can_place_on, can_break
    ".MTV", // 16: attribute_modifiers, custom_model_data, tooltip_display, repair_cost
    "UBN.", // 20: creative_slot_lock, enchantment_glint_override, intangible_projectile, food
    "...H", // 24: consumable, use_remainder, use_cooldown, damage_resistant
    "...V", // 28: tool, weapon, attack_range, enchantable
    ".HUS", // 32: equippable, repairable, glider, tooltip_style
    "....", // 36: death_protection, blocks_attacks, piercing_weapon, kinetic_weapon
    ".VeV", // 40: swing_animation, additional_trade_cost, stored_enchantments, dye
    "IIVN", // 44: dyed_color, map_color, map_id, map_decorations
    "V...", // 48: map_post_processing, charged_projectiles, bundle_contents, potion_contents
    "F...", // 52: potion_duration_scale, suspicious_stew_effects, writable_book_content, written_book_content
    ".N.N", // 56: trim, debug_stick_state, entity_data, bucket_entity_data
    "...V", // 60: block_entity_data, instrument, provides_trim_material, ominous_bottle_amplifier
    ".H..", // 64: jukebox_playable, provides_banner_patterns, recipes, lodestone_tracker
    "..pS", // 68: firework_explosion, fireworks, profile, note_block_sound
    ".Vv.", // 72: banner_patterns, base_color, pot_decorations, container
    "...N", // 76: block_state, bees, sulfur_cube_content, lock
    "N.VV", // 80: container_loot, break_sound, villager_variant, wolf_variant
    "VVVV", // 84: wolf_sound_variant, wolf_collar, fox_variant, salmon_size
    "VVVV", // 88: parrot_variant, tropical_fish_pattern, tropical_fish_base_color, tropical_fish_pattern_color
    "VVVV", // 92: mooshroom_variant, rabbit_variant, pig_variant, pig_sound_variant
    "VVVV", // 96: cow_variant, cow_sound_variant, chicken_variant, chicken_sound_variant
    "VVV.", // 100: zombie_nautilus_variant, frog_variant, horse_variant, painting_variant
    "VVVV", // 104: llama_variant, axolotl_variant, cat_variant, cat_sound_variant
    "VVV",  // 108: cat_collar, sheep_color, shulker_color
);

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
    /// Registerdaten; Items holen daraus Registry-Namen, die Live-Ansicht die Welthöhe.
    #[cfg(feature = "extras")]
    pub const CB_REGISTRY_DATA: i32 = 7;
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

/// Zahlenwerte in den Zusatzpaketen der Ausbaustufen. Alle aus denselben Klassen abgelesen wie
/// die Paket-IDs (Aufzählungsreihenfolge = übertragener Wert).
#[cfg(feature = "extras")]
pub mod values {
    /// `PlayerState` in 1.21.1 – dort werden Schleichen und Sprinten noch als Spielerbefehl
    /// geschickt. Ab 1.21.11 gibt es diese vier Werte nicht mehr; dort trägt das Eingabepaket
    /// die Zustände (siehe [`input`]).
    #[cfg(feature = "state")]
    pub mod player_state {
        pub const START_SNEAKING: i32 = 0;
        pub const STOP_SNEAKING: i32 = 1;
        pub const START_SPRINTING: i32 = 3;
        pub const STOP_SPRINTING: i32 = 4;
    }

    /// Bits im Eingabepaket ab 1.21.11 (`ServerboundPlayerInputPacket`, ein Byte).
    #[cfg(feature = "state")]
    pub mod input {
        pub const SNEAK: u8 = 0x20;
        pub const SPRINT: u8 = 0x40;
    }

    /// `ContainerActionType` – der Modus im Klick-Paket.
    #[cfg(feature = "menu")]
    pub mod click {
        /// Normaler Klick (Knopf 0 = links, 1 = rechts).
        pub const NORMAL: u8 = 0;
        /// Umschalt-Klick (Knopf 0).
        pub const SHIFT: u8 = 1;
    }

    /// `ScoreboardPosition` – uns interessiert nur die Seitenleiste.
    #[cfg(feature = "board")]
    pub const SIDEBAR: i32 = 1;

    /// `ObjectiveAction`
    #[cfg(feature = "board")]
    pub mod objective {
        pub const ADD: u8 = 0;
        pub const REMOVE: u8 = 1;
        pub const UPDATE: u8 = 2;
    }

    /// `TeamAction`
    #[cfg(feature = "board")]
    pub mod team {
        pub const CREATE: u8 = 0;
        pub const REMOVE: u8 = 1;
        pub const UPDATE: u8 = 2;
        pub const ADD_PLAYER: u8 = 3;
        pub const REMOVE_PLAYER: u8 = 4;
    }
}
