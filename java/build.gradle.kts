plugins {
    application
    id("com.gradleup.shadow") version "8.3.5"
}

group = "net.gravijet.afk"
version = "2.1.0"

// ============================================================================
// Version wählen:  -Pmc=1.21.1 | 1.21.11 | 26.1 | 26.2   (Standard: 26.1)
//
// MCProtocolLib spricht pro Build genau EINE Protokollversion – deshalb entsteht aus derselben
// Codebasis je Version ein eigenes, schlankes Jar:
//   ./gradlew :java:shadowJar -Pmc=26.1     ->  build/libs/afk-26.1.jar
// oder alle auf einmal über build-all.ps1 / build-all.sh.
// ============================================================================
val mc = (project.findProperty("mc") as String?) ?: "26.1"

// Die Protokoll-Bibliothek je Version. 1.21.1 und 26.2 gibt es (noch) nicht als Release:
//  * 1.21.1 -> der letzte 1.21-Snapshot (Protokoll 767, gilt für 1.21 und 1.21.1). Fest auf den
//    Zeitstempel gepinnt: der Zweig ist seit Oktober 2024 eingefroren, so bleibt der Build
//    reproduzierbar.
//  * 26.2  -> laufender Snapshot. Bewusst NICHT gepinnt, weil sich das Protokoll dort noch
//    ändert und ein 26.2-Server den jeweils neuesten Stand erwartet.
val protocolVersion = when (mc) {
    "1.21.1" -> "1.21-20241010.155958-24"
    "1.21.11" -> "1.21.11-1"
    "26.1" -> "26.1-1"
    "26.2" -> "26.2-SNAPSHOT"
    else -> throw GradleException("Unbekannte Version '$mc' (erlaubt: 1.21.1 | 1.21.11 | 26.1 | 26.2)")
}

// MCProtocolLib hat seine Netz-API mit 1.21.2 umgebaut (ClientSession/Factory statt
// TcpClientSession) und mehrere Pakete um Felder erweitert. Der Hauptcode kennt davon nichts: er
// ruft nur `Net`, und das liegt je Version in einem eigenen Quellordner.
val legacyApi = mc == "1.21.1"
val apiSourceDir = if (legacyApi) "src/api-legacy/java" else "src/api-modern/java"

// ============================================================================
// Bewegung zuschalten:  -Pmove=true   (Standard: aus)
//
// Ergibt je Version ein zweites Jar:
//   afk-26.1.jar        schlank, ohne jede Bewegung
//   afk-26.1-move.jar   zusätzlich :go / :look / :home
//
// Der Bewegungscode liegt in src/move/java und wird NUR dann einkompiliert; der Hauptcode lädt
// ihn rein reflektiv (siehe Mover.load()). Das schlanke Jar enthält davon keine Klasse.
// ============================================================================
val movementEnabled = (project.findProperty("move") as String?).toBoolean()
val moverSource = "src/move/java/net/gravijet/afk/move/Movement.java"
if (movementEnabled && !file(moverSource).exists()) {
    throw GradleException("-Pmove=true benötigt $moverSource – die Datei fehlt.")
}

val minecraftAuthVersion: String by project
val adventureVersion: String by project
val gsonVersion: String by project
val slf4jVersion: String by project
val junitVersion: String by project

repositories {
    mavenCentral()
    // MCProtocolLib (GeyserMC) – Releases und Snapshots (1.21.1 und 26.2 liegen dort).
    maven("https://repo.opencollab.dev/maven-releases/")
    maven("https://repo.opencollab.dev/maven-snapshots/")
    // MinecraftAuth (RaphiMC) + Lenni0451 commons
    maven("https://maven.lenni0451.net/releases")
    maven("https://maven.lenni0451.net/snapshots")
}

sourceSets {
    main {
        java {
            srcDir(apiSourceDir)
            if (movementEnabled) {
                srcDir("src/move/java")
            }
        }
    }
}

dependencies {
    implementation("org.geysermc.mcprotocollib:protocol:$protocolVersion")
    implementation("net.raphimc:MinecraftAuth:$minecraftAuthVersion")

    // Chat-Komponenten farbig im Terminal rendern (Adventure ist ohnehin transitive
    // MCProtocolLib-Abhängigkeit, die Serializer kosten daher fast nichts extra).
    implementation(platform("net.kyori:adventure-bom:$adventureVersion"))
    implementation("net.kyori:adventure-text-serializer-ansi")
    implementation("net.kyori:adventure-text-serializer-plain")

    implementation("com.google.code.gson:gson:$gsonVersion")
    implementation("org.slf4j:slf4j-simple:$slf4jVersion")

    testImplementation(platform("org.junit:junit-bom:$junitVersion"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

// Snapshots dürfen nicht tagelang aus dem Cache kommen – sonst baut die CI gegen einen alten
// Stand, während der Server längst weiter ist.
configurations.all {
    resolutionStrategy.cacheChangingModulesFor(0, "seconds")
}

java {
    toolchain {
        languageVersion.set(JavaLanguageVersion.of(21))
    }
}

// Die Quellen enthalten Umlaute. Ohne diese Angabe liest javac sie in der Standard-Kodierung der
// JVM – unter Windows ist das windows-1252, und der Build bricht ab.
tasks.withType<JavaCompile>().configureEach {
    options.encoding = "UTF-8"
    options.compilerArgs.addAll(listOf("-Xlint:all", "-Werror"))
}

tasks.test {
    useJUnitPlatform()
}

application {
    mainClass.set("net.gravijet.afk.Main")
}

tasks.shadowJar {
    archiveFileName.set("afk-$mc${if (movementEnabled) "-move" else ""}.jar")
    mergeServiceFiles()
    // Die Netty-QUIC-Natives (quiche, ~30 MB über 5 Plattformen) werden nie gebraucht –
    // Minecraft läuft über TCP, der Microsoft-Login über HTTPS. Rauswerfen = drastisch kleineres Jar.
    exclude("**/*quiche*")
    exclude("META-INF/native-image/io.netty/netty-codec-native-quic/**")
    manifest {
        // Der Client liest das zur Anzeige zurück (siehe Main.minecraftVersion()).
        attributes["Implementation-Version"] = mc
        attributes["Enable-Native-Access"] = "ALL-UNNAMED"
    }
}

tasks.named("build") {
    dependsOn("shadowJar")
}

gradle.taskGraph.whenReady {
    logger.lifecycle("AFKSystems-Build: MC $mc (MCProtocolLib $protocolVersion, Bewegung=${if (movementEnabled) "an" else "aus"})")
}
