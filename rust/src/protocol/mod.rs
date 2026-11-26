//! Die unterstützten Minecraft-Protokolle: Paket-IDs, Formatweichen und die Zuordnung
//! eingehender Pakete.
//!
//! Die IDs aller modernen Versionen stammen aus Mojangs eigenem Datengenerator
//! (`--reports` → `packets.json`, `registries.json`) und stehen in [`ids`], erzeugt von
//! `data/gen-protocol.py`. Handgeschrieben ist nur die 1.8.9-Tabelle (Protokoll 47, vor dem
//! Datengenerator), abgeglichen mit PrismarineJS/minecraft-data und dem 1.8.9-Client.
//!
//! Bei einem MC-Update: Bericht neu erzeugen, nicht raten.

pub mod buf;
mod ids;
pub mod item_names;
pub mod items;
pub mod nbt;

pub use ids::Ids;

/// Protokollzustand. Die Zahlenwerte sind nur intern.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Login,
    Configuration,
    Game,
}

/// Ein unterstütztes Protokoll: Name für die Kommandozeile, Versionsnummer für den Handshake,
/// Paket-IDs und die wenigen Formatweichen, an denen sich die Versionen unterscheiden.
pub struct Protocol {
    /// So heißt die Version auf der Kommandozeile (`--mc 26.1`).
    pub name: &'static str,
    /// Protokollnummer im Handshake.
    pub version: i32,
    /// Altes Protokoll 47: keine Konfigurationsphase, JSON-Komponenten, 32-Bit-KeepAlive und die
    /// vor der Flattening-Umstellung verwendeten Chunk-/Slot-Formate.
    pub legacy: bool,
    /// Paketformate ab 1.21.2: Prüfsumme im Chat-Paket, `globalIndex` im Spieler-Chat,
    /// Partikel-Status in den Client-Einstellungen, Positionspaket als Vektor, Verhaltenskodex,
    /// Chunk-Format (Höhenkarten als Liste, Palettendaten ohne Länge), `TeleportEntity`.
    pub modern: bool,
    /// Ab 26.3 trägt `AcceptTeleportation` die übernommene Position; das frühere zusätzliche
    /// `MovePlayer`-Echo nach jedem Teleport entfällt.
    pub teleport_ack_position: bool,
    /// Ab 26.4 handeln Client und Server den Sitzungsschlüssel per X25519 aus
    /// (`ClientboundKey` statt `ClientboundHello`, siehe [`crate::net::crypto`]), und
    /// Verschlüsselung und Kompression werden erst mit `ApplyOptions`/`LoginFinished` scharf.
    pub key_exchange: bool,
    /// Ab 26.1 steht in jedem Chunk-Abschnitt vor den Paletten **zweimal** ein Short
    /// (Block- und Flüssigkeitszähler), davor nur einmal.
    pub section_fluid_count: bool,
    /// Feldreihenfolge im Team-Paket.
    pub team_layout: TeamLayout,
    /// Wie sich jede Gegenstands-Komponente überspringen lässt (siehe [`items`]).
    pub item_shapes: &'static str,
    /// 1.8.9 kann mehrere Chunks in einem eigenen Sammelpaket schicken; sonst `-1`.
    pub cb_level_chunk_bulk: i32,
    pub ids: Ids,
}

/// Aufbau von `ClientboundSetPlayerTeamPacket` nach `name` und `action`, für die Aktionen
/// „anlegen" und „ändern". Abgelesen aus den vier Codec-Jars.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TeamLayout {
    /// 1.8.9: alle sichtbaren Texte sind alte Zeichenketten; Präfix/Suffix stehen direkt hinter
    /// dem Anzeigenamen, Farbe und Flags sind Bytes.
    V1_8,
    /// 1.21.1: Anzeigename, Flags (Byte), Sichtbarkeit **als Zeichenkette**, Kollision **als
    /// Zeichenkette**, Farbe (VarInt), Präfix, Suffix.
    Legacy,
    /// 1.21.11 und 26.1: wie [`TeamLayout::Legacy`], aber Sichtbarkeit und Kollision als VarInt.
    VarIntRules,
    /// 26.2: Anzeigename, **Präfix, Suffix**, Sichtbarkeit, Kollision, Farbe (optional), Flags.
    Reordered,
}

/// Eingehende Pakete der Login-Phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoginIn {
    Disconnect,
    /// Bis 26.3: RSA-Schlüssel des Servers (`ClientboundHello`).
    Hello,
    /// Ab 26.4: X25519-Schlüssel des Servers (`ClientboundKey`).
    Key,
    Finished,
    Compression,
    CustomQuery,
    CookieRequest,
    /// Ab 26.4: Verschlüsselung und/oder Kompression jetzt einschalten.
    ApplyOptions,
    Ignored,
}

/// Eingehende Pakete der Konfigurationsphase.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConfigIn {
    KeepAlive,
    Ping,
    Finish,
    SelectKnownPacks,
    CodeOfConduct,
    RegistryData,
    ResourcePackPush,
    StoreCookie,
    CookieRequest,
    Transfer,
    Disconnect,
    Ignored,
}

/// Eingehende Pakete der Spielphase, die der Client auswertet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
    ChunkBatchStart,
    ChunkBatchFinished,
    // Bewegung und Physik
    EntityMotion,
    Explode,
    Abilities,
    GameEvent,
    // Welt (Standard: Kollision, POV: Bild)
    LevelChunk,
    LevelChunkBulk,
    ForgetChunk,
    BlockUpdate,
    SectionBlocks,
    // Anzeigetafel
    Objective,
    Score,
    ResetScore,
    DisplayObjective,
    Team,
    // Menüs
    OpenScreen,
    ContainerContent,
    ContainerSlot,
    ContainerClose,
    PlayerInventory,
    // Nur Live-Ansicht
    LightUpdate,
    AddEntity,
    RemoveEntities,
    MoveEntityPos,
    MoveEntityPosRot,
    TeleportEntity,
    EntityPositionSync,
    Ignored,
}

pub const PROTOCOLS: &[Protocol] = &[P1_8_9, P1_21_1, P1_21_11, P26_1, P26_2, P26_3, P26_4];

pub const DEFAULT: &Protocol = &P26_1;

impl Protocol {
    pub fn find(name: &str) -> Option<&'static Protocol> {
        let name = name.trim();
        PROTOCOLS.iter().find(|p| p.name == name)
    }

    pub fn names() -> String {
        PROTOCOLS
            .iter()
            .map(|p| p.name)
            .collect::<Vec<_>>()
            .join(" | ")
    }

    pub fn login_incoming(&self, id: i32) -> LoginIn {
        let i = &self.ids;
        match id {
            _ if id < 0 => LoginIn::Ignored,
            _ if id == i.login_cb_disconnect => LoginIn::Disconnect,
            _ if id == i.login_cb_hello => LoginIn::Hello,
            _ if id == i.login_cb_key => LoginIn::Key,
            _ if id == i.login_cb_finished => LoginIn::Finished,
            _ if id == i.login_cb_compression => LoginIn::Compression,
            _ if id == i.login_cb_custom_query => LoginIn::CustomQuery,
            _ if id == i.login_cb_cookie_request => LoginIn::CookieRequest,
            _ if id == i.login_cb_apply_options => LoginIn::ApplyOptions,
            _ => LoginIn::Ignored,
        }
    }

    pub fn config_incoming(&self, id: i32) -> ConfigIn {
        let i = &self.ids;
        match id {
            _ if id < 0 => ConfigIn::Ignored,
            _ if id == i.config_cb_keep_alive => ConfigIn::KeepAlive,
            _ if id == i.config_cb_ping => ConfigIn::Ping,
            _ if id == i.config_cb_finish => ConfigIn::Finish,
            _ if id == i.config_cb_select_known_packs => ConfigIn::SelectKnownPacks,
            _ if id == i.config_cb_code_of_conduct => ConfigIn::CodeOfConduct,
            _ if id == i.config_cb_registry_data => ConfigIn::RegistryData,
            _ if id == i.config_cb_resource_pack_push => ConfigIn::ResourcePackPush,
            _ if id == i.config_cb_store_cookie => ConfigIn::StoreCookie,
            _ if id == i.config_cb_cookie_request => ConfigIn::CookieRequest,
            _ if id == i.config_cb_transfer => ConfigIn::Transfer,
            _ if id == i.config_cb_disconnect => ConfigIn::Disconnect,
            _ => ConfigIn::Ignored,
        }
    }

    /// Spielphasen-ID → Paketart. Die häufigen Pakete stehen vorn.
    pub fn incoming(&self, id: i32) -> In {
        let i = &self.ids;
        let table: [(i32, In); 41] = [
            (i.cb_keep_alive, In::KeepAlive),
            (i.cb_move_entity_pos, In::MoveEntityPos),
            (i.cb_move_entity_pos_rot, In::MoveEntityPosRot),
            (i.cb_entity_position_sync, In::EntityPositionSync),
            (i.cb_level_chunk, In::LevelChunk),
            (i.cb_light_update, In::LightUpdate),
            (i.cb_block_update, In::BlockUpdate),
            (i.cb_section_blocks_update, In::SectionBlocks),
            (i.cb_set_entity_motion, In::EntityMotion),
            (i.cb_chunk_batch_finished, In::ChunkBatchFinished),
            (i.cb_chunk_batch_start, In::ChunkBatchStart),
            (i.cb_system_chat, In::SystemChat),
            (i.cb_player_chat, In::PlayerChat),
            (i.cb_player_position, In::Position),
            (i.cb_ping, In::Ping),
            (i.cb_set_health, In::SetHealth),
            (i.cb_teleport_entity, In::TeleportEntity),
            (i.cb_add_entity, In::AddEntity),
            (i.cb_remove_entities, In::RemoveEntities),
            (i.cb_forget_level_chunk, In::ForgetChunk),
            (self.cb_level_chunk_bulk, In::LevelChunkBulk),
            (i.cb_set_score, In::Score),
            (i.cb_set_objective, In::Objective),
            (i.cb_reset_score, In::ResetScore),
            (i.cb_set_display_objective, In::DisplayObjective),
            (i.cb_set_player_team, In::Team),
            (i.cb_open_screen, In::OpenScreen),
            (i.cb_container_set_content, In::ContainerContent),
            (i.cb_container_set_slot, In::ContainerSlot),
            (i.cb_container_close, In::ContainerClose),
            (i.cb_set_player_inventory, In::PlayerInventory),
            (i.cb_explode, In::Explode),
            (i.cb_player_abilities, In::Abilities),
            (i.cb_game_event, In::GameEvent),
            (i.cb_login, In::Login),
            (i.cb_respawn, In::Respawn),
            (i.cb_disconnect, In::Disconnect),
            (i.cb_transfer, In::Transfer),
            (i.cb_resource_pack_push, In::ResourcePackPush),
            (i.cb_start_configuration, In::StartConfiguration),
            (i.cb_store_cookie, In::StoreCookie),
        ];
        if id < 0 {
            return In::Ignored;
        }
        if id == i.cb_cookie_request {
            return In::CookieRequest;
        }
        table
            .iter()
            .find(|(known, _)| *known == id)
            .map_or(In::Ignored, |(_, kind)| *kind)
    }
}

const P1_8_9: Protocol = Protocol {
    name: "1.8.9",
    version: 47,
    legacy: true,
    modern: false,
    teleport_ack_position: false,
    key_exchange: false,
    section_fluid_count: false,
    team_layout: TeamLayout::V1_8,
    item_shapes: "",
    cb_level_chunk_bulk: 0x26,
    ids: Ids {
        login_cb_disconnect: 0x00,
        login_cb_hello: 0x01,
        login_cb_finished: 0x02,
        login_cb_compression: 0x03,
        login_sb_hello: 0x00,
        login_sb_key: 0x01,
        cb_add_entity: 0x0c, // NamedEntitySpawn; die Live-Ansicht zeigt nur Spieler.
        cb_block_update: 0x23,
        cb_container_close: 0x2e,
        cb_container_set_content: 0x30,
        cb_container_set_slot: 0x2f,
        cb_disconnect: 0x40,
        cb_explode: 0x27,
        cb_game_event: 0x2b,
        cb_keep_alive: 0x00,
        cb_level_chunk: 0x21,
        cb_login: 0x01,
        cb_move_entity_pos: 0x15,
        cb_move_entity_pos_rot: 0x17,
        cb_open_screen: 0x2d,
        cb_player_abilities: 0x39,
        cb_player_position: 0x08,
        cb_remove_entities: 0x13,
        cb_resource_pack_push: 0x48,
        cb_respawn: 0x07,
        cb_section_blocks_update: 0x22,
        cb_set_display_objective: 0x3d,
        cb_set_entity_motion: 0x12,
        cb_set_health: 0x06,
        cb_set_objective: 0x3b,
        cb_set_player_team: 0x3e,
        cb_set_score: 0x3c, // Entfernen ist Aktion 1 im SetScore-Paket.
        cb_system_chat: 0x02,
        cb_teleport_entity: 0x18,
        sb_chat: 0x01,
        sb_chat_command: 0x01,
        sb_client_command: 0x16,
        sb_client_information: 0x15,
        sb_container_click: 0x0e,
        sb_container_close: 0x0d,
        sb_custom_payload: 0x17,
        sb_keep_alive: 0x00,
        sb_move_player_pos: 0x04,
        sb_move_player_pos_rot: 0x06,
        sb_move_player_rot: 0x05,
        sb_move_player_status_only: 0x03,
        sb_player_command: 0x0b,
        sb_player_input: 0x0c,
        sb_resource_pack: 0x19,
        sb_set_carried_item: 0x09,
        sb_swing: 0x0a,
        sb_use_item: 0x08,
        ..ids::NONE
    },
};

const P1_21_1: Protocol = Protocol {
    name: "1.21.1",
    version: 767,
    legacy: false,
    modern: false,
    teleport_ack_position: false,
    key_exchange: false,
    section_fluid_count: false,
    team_layout: TeamLayout::Legacy,
    item_shapes: SHAPES_1_21_1,
    cb_level_chunk_bulk: -1,
    ids: ids::P1_21_1,
};

const P1_21_11: Protocol = Protocol {
    name: "1.21.11",
    version: 774,
    legacy: false,
    modern: true,
    teleport_ack_position: false,
    key_exchange: false,
    section_fluid_count: false,
    team_layout: TeamLayout::VarIntRules,
    item_shapes: SHAPES_1_21_11,
    cb_level_chunk_bulk: -1,
    ids: ids::P1_21_11,
};

const P26_1: Protocol = Protocol {
    name: "26.1",
    version: 775,
    legacy: false,
    modern: true,
    teleport_ack_position: false,
    key_exchange: false,
    section_fluid_count: true,
    team_layout: TeamLayout::VarIntRules,
    item_shapes: SHAPES_26_1,
    cb_level_chunk_bulk: -1,
    ids: ids::P26_1,
};

const P26_2: Protocol = Protocol {
    name: "26.2",
    version: 776,
    legacy: false,
    modern: true,
    teleport_ack_position: false,
    key_exchange: false,
    section_fluid_count: true,
    team_layout: TeamLayout::Reordered,
    item_shapes: SHAPES_26_2,
    cb_level_chunk_bulk: -1,
    ids: ids::P26_2,
};

const P26_3: Protocol = Protocol {
    name: "26.3",
    version: 777,
    legacy: false,
    modern: true,
    teleport_ack_position: true,
    key_exchange: false,
    section_fluid_count: true,
    team_layout: TeamLayout::Reordered,
    item_shapes: SHAPES_26_3,
    cb_level_chunk_bulk: -1,
    ids: ids::P26_3,
};

/// 26.4 Pre-Release 1. Snapshots tragen Bit 30 in der Protokollnummer; mit dem Release ändert
/// sich nur diese Zahl (Bericht neu erzeugen).
const P26_4: Protocol = Protocol {
    name: "26.4",
    version: 0x4000_0000 | 349,
    legacy: false,
    modern: true,
    teleport_ack_position: true,
    key_exchange: true,
    section_fluid_count: true,
    team_layout: TeamLayout::Reordered,
    item_shapes: SHAPES_26_4,
    cb_level_chunk_bulk: -1,
    ids: ids::P26_4,
};

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

/// Aus den 26.2-Formen nach Komponentenname übernommen; neu hinzugekommene Komponenten
/// bleiben unbekannt (`.`), dann endet das Lesen dieses Gegenstands ehrlich dort.
const SHAPES_26_3: &str = concat!(
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
    "..V.", // 40: attack_animation, interact_animation, additional_trade_cost, block_transformer
    ".eVI", // 44: villager_food, stored_enchantments, dye, dyed_color
    "VNV.", // 48: map_id, map_decorations, map_post_processing, charged_projectiles
    "..F.", // 52: bundle_contents, potion_contents, potion_duration_scale, suspicious_stew_effects
    "...N", // 56: writable_book_content, written_book_content, trim, debug_stick_state
    ".N..", // 60: entity_data, bucket_entity_data, block_entity_data, instrument
    ".V.H", // 64: provides_trim_material, ominous_bottle_amplifier, jukebox_playable, provides_banner_patterns
    "....", // 68: recipes, lodestone_tracker, firework_explosion, fireworks
    "pS.V", // 72: profile, note_block_sound, banner_patterns, base_color
    "v...", // 76: pot_decorations, container, block_state, bees
    ".NN.", // 80: sulfur_cube_content, lock, container_loot, break_sound
    "....", // 84: compostable, cooking_fuel, brewing_fuel, mob_visibility
    "VVVV", // 88: villager/variant, wolf/variant, wolf/sound_variant, wolf/collar
    "VVVV", // 92: fox/variant, salmon/size, parrot/variant, tropical_fish/pattern
    "VVVV", // 96: tropical_fish/base_color, tropical_fish/pattern_color, mooshroom/variant, rabbit/variant
    "VVVV", // 100: pig/variant, pig/sound_variant, cow/variant, cow/sound_variant
    "VVVV", // 104: chicken/variant, chicken/sound_variant, zombie_nautilus/variant, frog/variant
    "V.VV", // 108: horse/variant, painting/variant, llama/variant, axolotl/variant
    "VVVV", // 112: cat/variant, cat/sound_variant, cat/collar, sheep/color
    "V...", // 116: shulker/color, provides_pottery_pattern, sign_text_front, sign_text_back
    "..",   // 120: waxed, cushion/color
);

/// Aus den 26.2-Formen nach Komponentenname übernommen; neu hinzugekommene Komponenten
/// bleiben unbekannt (`.`), dann endet das Lesen dieses Gegenstands ehrlich dort.
const SHAPES_26_4: &str = concat!(
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
    "..V.", // 40: attack_animation, interact_animation, additional_trade_cost, block_transformer
    "..eV", // 44: villager_food, dispense_behavior, stored_enchantments, dye
    "IVNV", // 48: dyed_color, map_id, map_decorations, map_post_processing
    "...F", // 52: charged_projectiles, bundle_contents, potion_contents, potion_duration_scale
    "....", // 56: suspicious_stew_effects, writable_book_content, written_book_content, trim
    "N.N.", // 60: debug_stick_state, entity_data, bucket_entity_data, block_entity_data
    "..V.", // 64: instrument, provides_trim_material, ominous_bottle_amplifier, jukebox_playable
    "H...", // 68: provides_banner_patterns, recipes, lodestone_tracker, firework_explosion
    ".pS.", // 72: fireworks, profile, note_block_sound, banner_patterns
    "Vv..", // 76: base_color, pot_decorations, container, block_state
    "..NN", // 80: bees, sulfur_cube_content, lock, container_loot
    "....", // 84: break_sound, compostable, cooking_fuel, brewing_fuel
    ".VVV", // 88: mob_visibility, villager/variant, wolf/variant, wolf/sound_variant
    "VVVV", // 92: wolf/collar, fox/variant, salmon/size, parrot/variant
    "VVVV", // 96: tropical_fish/pattern, tropical_fish/base_color, tropical_fish/pattern_color, mooshroom/variant
    "VVVV", // 100: rabbit/variant, pig/variant, pig/sound_variant, cow/variant
    "VVVV", // 104: cow/sound_variant, chicken/variant, chicken/sound_variant, zombie_nautilus/variant
    "VV.V", // 108: frog/variant, horse/variant, painting/variant, llama/variant
    "VVVV", // 112: axolotl/variant, cat/variant, cat/sound_variant, cat/collar
    "VV..", // 116: sheep/color, shulker/color, provides_pottery_pattern, sign_text_front
    "...",  // 120: sign_text_back, waxed, cushion/color
);

/// Handshake (nur ein Paket, in allen Versionen gleich).
pub mod handshake {
    pub const SB_INTENTION: i32 = 0;
    /// intent-Feld: 1=Status, 2=Login, 3=Transfer
    pub const INTENT_LOGIN: i32 = 2;
    pub const INTENT_TRANSFER: i32 = 3;
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
}
