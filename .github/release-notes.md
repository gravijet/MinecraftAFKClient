3.0.0 adds Minecraft 26.3 and 26.4, real player physics and far lower memory use.

Builds: There are now exactly two files per platform. `afk-*` is the standard client (chat, commands, macros, movement, scoreboard, menus, inventory, anti-AFK, browser menu); `pov-afk-*` adds the live view. The former slim, move, items, premium and ultra variants are gone; the panel unlocks features itself.

Versions: Minecraft 26.3 and 26.4 (pre-release 1) are supported. 26.4 uses the new login encryption: X25519 key exchange, HKDF-SHA256, separate keys and IVs per direction and an integrity check, verified against Mojang's own classes. Packet ids for all modern versions are generated from Mojang's reports.

Physics: The bot moves like a vanilla player. Gravity, ground friction per block, step height, jumping, sprinting, sneaking with edge protection, water, lava, ladders, cobwebs and slime are ported from the vanilla client, with collision shapes generated from the official server jar. Walking off an edge now falls instead of floating. `:go` presses keys and lets the physics move the player. Knockback and explosions push the bot as they would a real player.

Memory: The world is kept bit-packed, uniform sections cost nothing and textures load on demand. Measured on Linux (anonymous memory, 26.1 server): the standard client uses about 0.9 to 1.0 MB, the live view build about 9 MB instead of over 100 MB. The live view now requests view distance 3 by default; `--view-distance` asks for more.

Fixes: The code-of-conduct prompt is accepted once (MC-312695). On 26.3 and later the teleport confirmation is sent on the next client tick, as the vanilla client does; an immediate answer made 26.4 servers log "moved too quickly" in a loop at join.
