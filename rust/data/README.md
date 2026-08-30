# Item-ID-Listen

Die vier Textdateien ordnen die Zeilennummer (= `protocol_id`) dem Vanilla-Ressourcennamen eines
Items zu. Sie werden ausschließlich in Rust-Bauformen mit dem Cargo-Feature `items` eingebettet.

Quelle ist jeweils der offizielle Mojang-Server-JAR derselben Version. Die Listen wurden mit dem
Mojang-Datengenerator (`net.minecraft.data.Main --reports`) aus
`reports/registries.json` → `minecraft:item.entries` erzeugt und numerisch nach `protocol_id`
sortiert. Verwendete Server-JARs:

| Version | SHA-1 | Einträge |
| --- | --- | ---: |
| 1.21.1 | `59353fb40c36d304f2035d51e7d6e6baa98dc05c` | 1333 |
| 1.21.11 | `64bb6d763bed0a9f1d632ec347938594144943ed` | 1505 |
| 26.1 | `3872a7f07a1a595e651aef8b058dfc2bb3772f46` | 1506 |
| 26.2 | `823e2250d24b3ddac457a60c92a6a941943fcd6a` | 1537 |

Bei einer neuen Protokollversion muss eine neue Liste aus genau deren Server-JAR erzeugt werden;
IDs aus einer anderen Version dürfen nicht übernommen oder geraten werden.

## Block-State-Tabellen der Browser-POV

`block-states-<version>.txt.gz` ordnet jede globale Netzwerk-State-ID dem Ressourcennamen, den
Properties und Mojangs `default`-Markierung des Blockzustands zu. Die Quelle sind dieselben vier
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
