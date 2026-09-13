# Item-ID-Listen

Die fünf Textdateien ordnen die Netzwerk-ID dem Vanilla-Ressourcennamen eines Items zu. Sie werden
ausschließlich in Rust-Bauformen mit dem Cargo-Feature `items` eingebettet. Bei 1.8.9 besteht der
Schlüssel aus `numerische ID:Metadatenwert`; bei den neueren Versionen ist die Zeilennummer die
Registry-`protocol_id`.

Quelle der vier modernen Listen ist jeweils die offizielle Mojang-Server-JAR derselben Version. Sie wurden mit dem
Mojang-Datengenerator (`net.minecraft.data.Main --reports`) aus
`reports/registries.json` → `minecraft:item.entries` erzeugt und numerisch nach `protocol_id`
sortiert. Verwendete Server-JARs:

| Version | SHA-1 | Einträge |
| --- | --- | ---: |
| 1.21.1 | `59353fb40c36d304f2035d51e7d6e6baa98dc05c` | 1333 |
| 1.21.11 | `64bb6d763bed0a9f1d632ec347938594144943ed` | 1505 |
| 26.1 | `3872a7f07a1a595e651aef8b058dfc2bb3772f46` | 1506 |
| 26.2 | `823e2250d24b3ddac457a60c92a6a941943fcd6a` | 1537 |

Minecraft 1.8.9 besitzt noch keinen Mojang-Datengenerator und keine Registry-Reports. Seine 581
gültigen ID-/Metadatenpaare stammen deshalb aus PrismarineJS `minecraft-data`, Dateien
`data/pc/1.8/items.json` (SHA-256
`a4efc1a0044cfa1996507502cfde5e60a330e2a6f97b7d8d65268819788ecd48`) und
`data/pc/common/legacy.json` (SHA-256
`0d7aec9a3107a62e322f8839ea2b96d24e2945c461c7aa29c6a77f854609a2e9`). Die erzeugte
`items-1.8.9.txt` hat SHA-256
`2eeec4efc85420033e3989b16a0132e11a2fd90dec87f63b4a10e7f68c044ac8`.

Bei einer neuen Protokollversion muss die Liste aus genau deren Versionsdaten erzeugt werden;
IDs aus einer anderen Version dürfen nicht übernommen oder geraten werden.

## Block-State-Tabellen der Browser-POV

`block-states-<version>.txt.gz` ordnet jede globale Netzwerk-State-ID dem Ressourcennamen, den
Properties und der `default`-Markierung des Blockzustands zu. Die Quelle sind bei den modernen
Versionen dieselben vier
offiziellen Server-JARs und darin `generated/reports/blocks.json`. Der Client braucht diese
Zuordnung, weil Chunk-Pakete nur die ID, die Client-JAR Modelle und Texturen dagegen nur
Ressourcennamen enthält. Die Default-Markierung bestimmt insbesondere das Modell eines Blockitems;
die kleinste State-ID ist bei vielen Blocks nicht der Default-State.

Die Dateien werden mit `generate-block-states.sh` reproduzierbar neu erzeugt. Das Skript lädt die
zu den Versionsmetadaten gehörenden offiziellen Server-JARs, startet Mojangs Datengenerator und
sortiert lückenlos nach State-ID und verifiziert vor dem Start den von Mojang veröffentlichten
SHA-1-Hash der Server-JAR. Benötigt werden `curl`, `jq`, `gzip`, `sha1sum` und ein zur Server-JAR
passendes Java. Die Texturen selbst sind ausdrücklich nicht enthalten; sie kommen zur Laufzeit
aus der mit `--pov-resources` angegebenen Client-JAR des Nutzers.

Für 1.8.9 ist die globale State-ID direkt als `Block-ID << 4 | Metadatenwert` definiert. Die
Tabelle enthält daher genau 4.096 Zeilen und wurde aus `data/pc/1.8/blocks.json` (SHA-256
`efc19622bb57292685d199bb341cae8c6bf82f1c700b157755c4a4edf9202aa0`) zusammen mit der obigen
Legacy-Zuordnung erzeugt. Ihre komprimierte SHA-256 ist
`65a19d1e77ca35c4725104d195f6746c095b50ece4192b3f339d3b55c3fdfc05`. Gegen die unveränderte
offizielle 1.8.9-Client-JAR (Mojang-SHA-1
`3870888a6c3d349d3771a3e9d16c9bf5e076b908`) wird das Laden von Blockmodellen und alten
`models/item`-/`textures/items`-Ressourcen im Integrationstest geprüft.
