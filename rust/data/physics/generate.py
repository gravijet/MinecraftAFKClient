#!/usr/bin/env python3
"""Build the block physics tables the client embeds (data/physics/*.bin).

Inputs
  <reports>/<v>/physics.tsv   output of Dump.java for every unobfuscated version (26.1+)
  data/block-states-<v>.txt.gz for the older versions (name + properties per state id)

Older versions (1.8.9, 1.21.1, 1.21.11) have no unobfuscated jar; their states are matched by
block name and properties against the 26.1 dump.

Output
  classes.bin  every distinct movement class: friction, speed and jump factor, flags, fluid
               amount and collision boxes
  <v>.bin      per state id the class index (u16, little endian)

Usage: python3 data/physics/generate.py <reports-dir>
"""
import gzip
import pathlib
import struct
import sys

HERE = pathlib.Path(__file__).resolve().parent
DATA = HERE.parent

DUMPED = {"26.1": "26.1", "26.2": "26.2", "26.3": "26.3", "26.4": "26.4-pre-1"}
MATCHED = ["1.8.9", "1.21.1", "1.21.11"]

# flag bits (must match src/physics/blocks.rs)
WATER, LAVA, FALLING, CLIMBABLE, COBWEB, BERRY_BUSH, POWDER_SNOW, SLIME, BED, HONEY, \
    SCAFFOLDING, BUBBLE_COLUMN, SOUL_SAND = (1 << i for i in range(13))

CLIMBABLE_BLOCKS = {
    "ladder", "vine", "scaffolding", "twisting_vines", "twisting_vines_plant",
    "weeping_vines", "weeping_vines_plant", "cave_vines", "cave_vines_plant",
}
NAMED = {
    "cobweb": COBWEB,
    "sweet_berry_bush": BERRY_BUSH,
    "powder_snow": POWDER_SNOW,
    "slime_block": SLIME,
    "honey_block": HONEY,
    "scaffolding": SCAFFOLDING,
    "bubble_column": BUBBLE_COLUMN,
    "soul_sand": SOUL_SAND,
}


def parse_dump(path):
    states = []
    for line in path.read_text().splitlines():
        sid, name, props, friction, speed, jump, fluid, boxes = line.split("\t")
        flags = NAMED.get(name, 0)
        if name in CLIMBABLE_BLOCKS:
            flags |= CLIMBABLE
        if name.endswith("_bed"):
            flags |= BED
        amount = 0
        if fluid != "-":
            kind, level = fluid.split(":")
            flags |= WATER if kind == "water" else LAVA
            if level.endswith("f"):
                flags |= FALLING
                level = level[:-1]
            amount = int(level)
        box_list = tuple(
            tuple(float(v) for v in b.split(",")) for b in boxes.split(";") if b
        )
        cls = (float(friction), float(speed), float(jump), flags, amount, box_list)
        states.append((int(sid), name, props, cls))
    assert [s[0] for s in states] == list(range(len(states)))
    return states


def parse_table(version):
    rows = []
    with gzip.open(DATA / f"block-states-{version}.txt.gz", "rt") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line:
                continue
            sid, name, props, _default = line.split("\t")
            rows.append((int(sid), name.split(":")[-1], props))
    return rows


# 1.8.9 names that the state table still carries in their pre-flattening spelling
LEGACY_NAMES = {
    "grass": "grass_block", "planks": "oak_planks", "sapling": "oak_sapling",
    "flowing_water": "water", "flowing_lava": "lava", "log": "oak_log", "log2": "acacia_log",
    "noteblock": "note_block", "bed": "red_bed", "golden_rail": "powered_rail", "web": "cobweb",
    "tallgrass": "short_grass", "deadbush": "dead_bush", "wool": "white_wool",
    "piston_extension": "moving_piston", "yellow_flower": "dandelion", "red_flower": "poppy",
    "double_stone_slab": "smooth_stone", "double_stone_slab2": "red_sandstone",
    "brick_block": "bricks", "mob_spawner": "spawner", "lit_furnace": "furnace",
    "standing_sign": "oak_sign", "wall_sign": "oak_wall_sign", "wooden_door": "oak_door",
    "wooden_pressure_plate": "oak_pressure_plate", "lit_redstone_ore": "redstone_ore",
    "unlit_redstone_torch": "redstone_torch", "snow_layer": "snow", "reeds": "sugar_cane",
    "fence": "oak_fence", "portal": "nether_portal", "lit_pumpkin": "jack_o_lantern",
    "unpowered_repeater": "repeater", "powered_repeater": "repeater",
    "stained_glass": "white_stained_glass", "trapdoor": "oak_trapdoor",
    "monster_egg": "infested_stone", "stonebrick": "stone_bricks", "melon_block": "melon",
    "fence_gate": "oak_fence_gate", "waterlily": "lily_pad", "nether_brick": "nether_bricks",
    "lit_redstone_lamp": "redstone_lamp", "double_wooden_slab": "oak_planks",
    "wooden_slab": "oak_slab", "wooden_button": "oak_button", "skull": "skeleton_skull",
    "unpowered_comparator": "comparator", "powered_comparator": "comparator",
    "quartz_ore": "nether_quartz_ore", "stained_hardened_clay": "white_terracotta",
    "stained_glass_pane": "white_stained_glass_pane", "leaves2": "acacia_leaves",
    "slime": "slime_block", "carpet": "white_carpet", "hardened_clay": "terracotta",
    "double_plant": "sunflower", "standing_banner": "white_banner",
    "wall_banner": "white_wall_banner", "stone_slab2": "red_sandstone_slab",
    # renamed after 1.21.1
    "chain": "iron_chain",
}


def props_dict(text):
    return dict(p.split("=", 1) for p in text.split(",") if p)


def match(rows, reference):
    by_name = {}
    for _, name, props, cls in reference:
        by_name.setdefault(name, []).append((props_dict(props), cls))
    misses = 0
    size = max(r[0] for r in rows) + 1
    table = [None] * size
    for sid, name, props in rows:
        name = name if name in by_name else LEGACY_NAMES.get(name, name)
        wanted = props_dict(props)
        candidates = by_name.get(name, [])
        best = None
        for have, cls in candidates:
            if all(have.get(k) == v for k, v in wanted.items()):
                extra_ok = have.get("waterlogged", "false") == "false"
                if best is None or (extra_ok and not best[0]):
                    best = (extra_ok, cls)
        if best is None and candidates:
            best = (False, candidates[0][1])
        if best is None:
            misses += 1
            # Unknown block: a full cube is the safe guess (the server would stop us anyway).
            best = (False, (0.6, 1.0, 1.0, 0, 0, ((0.0, 0.0, 0.0, 1.0, 1.0, 1.0),)))
        table[sid] = best[1]
    air = next(cls for _, name, _, cls in reference if name == "air")
    table = [cls if cls is not None else air for cls in table]
    return table, misses


def main():
    reports = pathlib.Path(sys.argv[1])
    per_version = {}
    dumps = {}
    for name, folder in DUMPED.items():
        dumps[name] = parse_dump(reports / folder / "physics.tsv")
        per_version[name] = [s[3] for s in dumps[name]]
    for name in MATCHED:
        table, misses = match(parse_table(name), dumps["26.1"])
        per_version[name] = table
        print(f"{name}: {len(table)} states, {misses} unmatched")

    classes, index = [], {}
    for name in ["1.8.9", "1.21.1", "1.21.11", "26.1", "26.2", "26.3", "26.4"]:
        for cls in per_version[name]:
            if cls not in index:
                index[cls] = len(classes)
                classes.append(cls)

    blob = bytearray(struct.pack("<H", len(classes)))
    for friction, speed, jump, flags, amount, boxes in classes:
        blob += struct.pack("<fffHBB", friction, speed, jump, flags, amount, len(boxes))
        for box in boxes:
            blob += struct.pack("<6f", *box)
    (HERE / "classes.bin").write_bytes(bytes(blob))

    # One little-endian u16 class index per state id. Kept uncompressed on purpose: embedded
    # read-only data is mapped from the executable and costs no heap memory at run time.
    for name, table in per_version.items():
        data = b"".join(struct.pack("<H", index[cls]) for cls in table)
        (HERE / f"{name}.bin").write_bytes(data)
        print(f"{name}: {len(table)} states")
    print(f"{len(classes)} classes")


if __name__ == "__main__":
    main()
