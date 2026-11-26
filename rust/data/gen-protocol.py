#!/usr/bin/env python3
"""Generate src/protocol/ids.rs from Mojang's own data-generator reports.

Every modern version is read from `<reports>/<version>/generated/reports/{packets,registries}.json`,
produced by `java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports`
(see data/README.md). Only maintainers need this; the output is committed.

Usage: python3 data/gen-protocol.py <reports-dir>
"""
import json
import pathlib
import sys

# (Rust name, CLI name, protocol version, report directory)
VERSIONS = [
    ("P1_21_1", "1.21.1", 767, "1.21.1"),
    ("P1_21_11", "1.21.11", 774, "1.21.11"),
    ("P26_1", "26.1", 775, "26.1"),
    ("P26_2", "26.2", 776, "26.2"),
    ("P26_3", "26.3", 777, "26.3"),
    ("P26_4", "26.4", 0x40000000 | 349, "26.4-pre-1"),
]

# field -> (phase, direction, [packet names, first match wins]); missing everywhere -> -1
FIELDS = {
    # --- login ---
    "login_cb_disconnect": ("login", "clientbound", ["login_disconnect"]),
    "login_cb_hello": ("login", "clientbound", ["hello"]),
    "login_cb_key": ("login", "clientbound", ["key"]),
    "login_cb_finished": ("login", "clientbound", ["login_finished", "game_profile"]),
    "login_cb_compression": ("login", "clientbound", ["login_compression", "setup_compression"]),
    "login_cb_custom_query": ("login", "clientbound", ["custom_query"]),
    "login_cb_cookie_request": ("login", "clientbound", ["cookie_request"]),
    "login_cb_apply_options": ("login", "clientbound", ["apply_options"]),
    "login_sb_hello": ("login", "serverbound", ["hello"]),
    "login_sb_key": ("login", "serverbound", ["key"]),
    "login_sb_custom_query_answer": ("login", "serverbound", ["custom_query_answer"]),
    "login_sb_acknowledged": ("login", "serverbound", ["login_acknowledged"]),
    "login_sb_cookie_response": ("login", "serverbound", ["cookie_response"]),
    "login_sb_options_applied": ("login", "serverbound", ["options_applied"]),
    # --- configuration ---
    "config_cb_cookie_request": ("configuration", "clientbound", ["cookie_request"]),
    "config_cb_disconnect": ("configuration", "clientbound", ["disconnect"]),
    "config_cb_finish": ("configuration", "clientbound", ["finish_configuration"]),
    "config_cb_keep_alive": ("configuration", "clientbound", ["keep_alive"]),
    "config_cb_ping": ("configuration", "clientbound", ["ping"]),
    "config_cb_registry_data": ("configuration", "clientbound", ["registry_data"]),
    "config_cb_resource_pack_push": ("configuration", "clientbound", ["resource_pack_push"]),
    "config_cb_store_cookie": ("configuration", "clientbound", ["store_cookie"]),
    "config_cb_transfer": ("configuration", "clientbound", ["transfer"]),
    "config_cb_select_known_packs": ("configuration", "clientbound", ["select_known_packs"]),
    "config_cb_code_of_conduct": ("configuration", "clientbound", ["code_of_conduct"]),
    "config_sb_client_information": ("configuration", "serverbound", ["client_information"]),
    "config_sb_cookie_response": ("configuration", "serverbound", ["cookie_response"]),
    "config_sb_custom_payload": ("configuration", "serverbound", ["custom_payload"]),
    "config_sb_finish": ("configuration", "serverbound", ["finish_configuration"]),
    "config_sb_keep_alive": ("configuration", "serverbound", ["keep_alive"]),
    "config_sb_pong": ("configuration", "serverbound", ["pong"]),
    "config_sb_resource_pack": ("configuration", "serverbound", ["resource_pack"]),
    "config_sb_select_known_packs": ("configuration", "serverbound", ["select_known_packs"]),
    "config_sb_accept_code_of_conduct": ("configuration", "serverbound", ["accept_code_of_conduct"]),
    # --- play, clientbound ---
    "cb_add_entity": ("play", "clientbound", ["add_entity"]),
    "cb_block_update": ("play", "clientbound", ["block_update"]),
    "cb_chunk_batch_finished": ("play", "clientbound", ["chunk_batch_finished"]),
    "cb_chunk_batch_start": ("play", "clientbound", ["chunk_batch_start"]),
    "cb_container_close": ("play", "clientbound", ["container_close"]),
    "cb_container_set_content": ("play", "clientbound", ["container_set_content"]),
    "cb_container_set_slot": ("play", "clientbound", ["container_set_slot"]),
    "cb_cookie_request": ("play", "clientbound", ["cookie_request"]),
    "cb_disconnect": ("play", "clientbound", ["disconnect"]),
    "cb_entity_position_sync": ("play", "clientbound", ["entity_position_sync"]),
    "cb_explode": ("play", "clientbound", ["explode"]),
    "cb_forget_level_chunk": ("play", "clientbound", ["forget_level_chunk"]),
    "cb_game_event": ("play", "clientbound", ["game_event"]),
    "cb_keep_alive": ("play", "clientbound", ["keep_alive"]),
    "cb_level_chunk": ("play", "clientbound", ["level_chunk_with_light"]),
    "cb_light_update": ("play", "clientbound", ["light_update"]),
    "cb_login": ("play", "clientbound", ["login"]),
    "cb_move_entity_pos": ("play", "clientbound", ["move_entity_pos"]),
    "cb_move_entity_pos_rot": ("play", "clientbound", ["move_entity_pos_rot"]),
    "cb_open_screen": ("play", "clientbound", ["open_screen"]),
    "cb_ping": ("play", "clientbound", ["ping"]),
    "cb_player_abilities": ("play", "clientbound", ["player_abilities"]),
    "cb_player_chat": ("play", "clientbound", ["player_chat"]),
    "cb_player_position": ("play", "clientbound", ["player_position"]),
    "cb_remove_entities": ("play", "clientbound", ["remove_entities"]),
    "cb_reset_score": ("play", "clientbound", ["reset_score"]),
    "cb_resource_pack_push": ("play", "clientbound", ["resource_pack_push"]),
    "cb_respawn": ("play", "clientbound", ["respawn"]),
    "cb_section_blocks_update": ("play", "clientbound", ["section_blocks_update"]),
    "cb_set_display_objective": ("play", "clientbound", ["set_display_objective"]),
    "cb_set_entity_motion": ("play", "clientbound", ["set_entity_motion"]),
    "cb_set_health": ("play", "clientbound", ["set_health"]),
    "cb_set_objective": ("play", "clientbound", ["set_objective"]),
    "cb_set_player_inventory": ("play", "clientbound", ["set_player_inventory"]),
    "cb_set_player_team": ("play", "clientbound", ["set_player_team"]),
    "cb_set_score": ("play", "clientbound", ["set_score"]),
    "cb_start_configuration": ("play", "clientbound", ["start_configuration"]),
    "cb_store_cookie": ("play", "clientbound", ["store_cookie"]),
    "cb_system_chat": ("play", "clientbound", ["system_chat"]),
    "cb_teleport_entity": ("play", "clientbound", ["teleport_entity"]),
    "cb_transfer": ("play", "clientbound", ["transfer"]),
    # --- play, serverbound ---
    "sb_accept_teleportation": ("play", "serverbound", ["accept_teleportation"]),
    "sb_chat": ("play", "serverbound", ["chat"]),
    "sb_chat_ack": ("play", "serverbound", ["chat_ack"]),
    "sb_chat_command": ("play", "serverbound", ["chat_command"]),
    "sb_chunk_batch_received": ("play", "serverbound", ["chunk_batch_received"]),
    "sb_client_command": ("play", "serverbound", ["client_command"]),
    "sb_client_information": ("play", "serverbound", ["client_information"]),
    "sb_client_tick_end": ("play", "serverbound", ["client_tick_end"]),
    "sb_configuration_acknowledged": ("play", "serverbound", ["configuration_acknowledged"]),
    "sb_container_click": ("play", "serverbound", ["container_click"]),
    "sb_container_close": ("play", "serverbound", ["container_close"]),
    "sb_cookie_response": ("play", "serverbound", ["cookie_response"]),
    "sb_custom_payload": ("play", "serverbound", ["custom_payload"]),
    "sb_keep_alive": ("play", "serverbound", ["keep_alive"]),
    "sb_move_player_pos": ("play", "serverbound", ["move_player_pos"]),
    "sb_move_player_pos_rot": ("play", "serverbound", ["move_player_pos_rot"]),
    "sb_move_player_rot": ("play", "serverbound", ["move_player_rot"]),
    "sb_move_player_status_only": ("play", "serverbound", ["move_player_status_only"]),
    "sb_player_command": ("play", "serverbound", ["player_command"]),
    "sb_player_input": ("play", "serverbound", ["player_input"]),
    "sb_player_loaded": ("play", "serverbound", ["player_loaded"]),
    "sb_pong": ("play", "serverbound", ["pong"]),
    "sb_resource_pack": ("play", "serverbound", ["resource_pack"]),
    "sb_set_carried_item": ("play", "serverbound", ["set_carried_item"]),
    "sb_swing": ("play", "serverbound", ["swing"]),
    "sb_punch": ("play", "serverbound", ["punch"]),
    "sb_use_item": ("play", "serverbound", ["use_item"]),
}

REGISTRY_FIELDS = {
    "player_entity_type": ("minecraft:entity_type", "minecraft:player"),
    "component_custom_name": ("minecraft:data_component_type", "minecraft:custom_name"),
    "component_item_name": ("minecraft:data_component_type", "minecraft:item_name"),
    "component_lore": ("minecraft:data_component_type", "minecraft:lore"),
}


def ids_for(reports: pathlib.Path):
    packets = json.loads((reports / "packets.json").read_text())
    registries = json.loads((reports / "registries.json").read_text())
    out = {}
    for field, (phase, direction, names) in FIELDS.items():
        table = packets.get(phase, {}).get(direction, {})
        value = -1
        for name in names:
            entry = table.get("minecraft:" + name)
            if entry is not None:
                value = entry["protocol_id"]
                break
        out[field] = value
    for field, (registry, key) in REGISTRY_FIELDS.items():
        out[field] = registries[registry]["entries"][key]["protocol_id"]
    return out


def main():
    root = pathlib.Path(sys.argv[1])
    lines = [
        "// Generated by data/gen-protocol.py from Mojang's --reports output. Do not edit.",
        "// Regenerate after a Minecraft update instead of changing numbers by hand.",
        "",
        "/// Packet ids of one protocol version; `-1` = the version has no such packet.",
        "/// The last four fields are registry ids (entity type, item components).",
        "#[derive(Clone, Copy)]",
        "pub struct Ids {",
    ]
    names = list(FIELDS) + list(REGISTRY_FIELDS)
    lines += [f"    pub {name}: i32," for name in names]
    lines += ["}", "", "/// Every id unknown; the starting point for the hand-written 1.8.9 table.",
              "pub const NONE: Ids = Ids {"]
    lines += [f"    {name}: -1," for name in names]
    lines += ["};", ""]
    for const, name, version, folder in VERSIONS:
        ids = ids_for(root / folder / "generated" / "reports")
        lines.append(f"/// Minecraft {name} (protocol {version}), from the `{folder}` server reports.")
        lines.append(f"pub const {const}: Ids = Ids {{")
        for field, value in ids.items():
            lines.append(f"    {field}: {value},")
        lines.append("};")
        lines.append("")
    target = pathlib.Path(__file__).resolve().parent.parent / "src" / "protocol" / "ids.rs"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("\n".join(lines))
    print(f"wrote {target}")


if __name__ == "__main__":
    main()
