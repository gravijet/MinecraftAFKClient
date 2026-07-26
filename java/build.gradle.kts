plugins {
    application
    id("com.gradleup.shadow") version "8.3.5"
}

group = "net.gravijet.afk"
version = "1.0.0"

// ============================================================================
// Variante wählen:  -Pvariant=26.1 | 1.21.11 | 1.8.9   (Standard: 26.1)
//
// Aus EINER Codebasis entstehen drei schlanke Per-Version-Jars. MCProtocolLib
// spricht pro Build nur EINE Protokollversion, daher werden 1.21.11 und 26.1
// nativ gebaut (klein, leicht). Nur 1.8.9 hat keine native Client-Bibliothek und
// nutzt deshalb den ViaVersion-Stack (ViaLoader) zur Übersetzung native->1.8.
//   ./gradlew shadowJar -Pvariant=26.1
//   ./gradlew shadowJar -Pvariant=1.21.11
//   ./gradlew shadowJar -Pvariant=1.8.9
// oder alle drei via build-all.ps1 / build-all.sh
// ============================================================================
val variant = (project.findProperty("variant") as String?) ?: "26.1"

val minecraftAuthVersion: String by project
val adventureVersion: String by project
val jlineVersion: String by project
val gsonVersion: String by project
val slf4jVersion: String by project
val viaLoaderVersion: String by project
val viaNativeMcProtocolLibVersion: String by project

// Native MCProtocolLib-Protokollversion je Variante.
val mcProtocolLibVersion = when (variant) {
    "1.21.11" -> "1.21.11-1"
    "1.8.9" -> viaNativeMcProtocolLibVersion // native Version, die ViaLoader unterstützt
    "26.1" -> "26.1-1"
    else -> throw GradleException("Unbekannte variant='$variant' (erlaubt: 26.1 | 1.21.11 | 1.8.9)")
}
val viaEnabled = variant == "1.8.9"

// Schutz gegen ein fehletikettiertes Jar: 1.8.9 braucht die Via-Bridge. Fehlt sie, würde sonst
// ein natives 1.21.11-Jar unter dem Namen "1.8.9" entstehen (verbindet sich in Wahrheit als 1.21.11).
if (viaEnabled && !file("src/via/java/net/gravijet/afk/via/ViaProtocolBridge.java").exists()) {
    throw GradleException(
        "variant=1.8.9 benötigt die Via-Bridge (src/via/java/.../ViaProtocolBridge.java), die noch fehlt. " +
                "Baue vorerst -Pvariant=26.1 oder -Pvariant=1.21.11."
    )
}

repositories {
    mavenCentral()
    // MCProtocolLib (GeyserMC)
    maven("https://repo.opencollab.dev/maven-releases/")
    maven("https://repo.opencollab.dev/maven-snapshots/")
    // MinecraftAuth (RaphiMC) + Lenni0451 commons
    maven("https://maven.lenni0451.net/releases")
    maven("https://maven.lenni0451.net/snapshots")
    // ViaVersion / ViaLoader (nur für die 1.8.9-Variante)
    maven("https://repo.viaversion.com")
}

// Der Via-Anbindungscode liegt in src/via/java und wird NUR in die 1.8.9-Variante
// einkompiliert. Die modernen Jars bleiben dadurch komplett Via-frei. Der Hauptcode
// lädt ViaProtocolBridge rein reflektiv (siehe ProtocolBridge.load()), es gibt keine
// Compile-Abhängigkeit vom Hauptcode auf Via.
sourceSets {
    main {
        java {
            if (viaEnabled) {
                srcDir("src/via/java")
            }
        }
    }
}

dependencies {
    implementation("org.geysermc.mcprotocollib:protocol:$mcProtocolLibVersion")
    implementation("net.raphimc:MinecraftAuth:$minecraftAuthVersion")

    // Chat-Komponenten farbig im Terminal rendern (Adventure ist ohnehin transitive
    // MCProtocolLib-Abhängigkeit, die Serializer kosten daher fast nichts extra).
    implementation(platform("net.kyori:adventure-bom:$adventureVersion"))
    implementation("net.kyori:adventure-text-serializer-ansi")
    implementation("net.kyori:adventure-text-serializer-plain")

    // Eingabezeile getrennt vom scrollenden Output
    implementation("org.jline:jline:$jlineVersion")

    implementation("com.google.code.gson:gson:$gsonVersion")
    implementation("org.slf4j:slf4j-simple:$slf4jVersion")

    // Nur 1.8.9: ViaLoader zieht ViaVersion + ViaBackwards + ViaRewind + ViaLegacy
    // transitiv nach und übersetzt native->1.8.
    if (viaEnabled) {
        implementation("net.raphimc:ViaLoader:$viaLoaderVersion")
    }
}

java {
    toolchain {
        languageVersion.set(JavaLanguageVersion.of(21))
    }
}

// Die Quellen enthalten Umlaute und Rahmenzeichen. Ohne diese Angabe liest javac sie in der
// Standard-Kodierung der JVM – unter Windows/JDK 17 ist das windows-1252, und der Build bricht ab.
tasks.withType<JavaCompile>().configureEach {
    options.encoding = "UTF-8"
}

application {
    mainClass.set("net.gravijet.afk.Main")
    // Zur Laufzeit an den Client durchgereicht, damit das Menü die Version anzeigt.
    applicationDefaultJvmArgs = listOf("-Dhugoafk.variant=$variant")
}

tasks.shadowJar {
    archiveFileName.set("hugoafk-$variant.jar")
    mergeServiceFiles()
    // Via nutzt viel Reflection/Mappings – NICHT minimieren, sonst fehlen Klassen.
    // Aber: die Netty-QUIC-Natives (quiche, ~30 MB über 5 Plattformen) werden nie gebraucht –
    // Minecraft läuft über TCP, der Microsoft-Login über HTTPS. Rauswerfen = drastisch kleinere Jar.
    exclude("**/*quiche*")
    exclude("META-INF/native-image/io.netty/netty-codec-native-quic/**")
    manifest {
        attributes["Implementation-Version"] = variant
        attributes["Enable-Native-Access"] = "ALL-UNNAMED"
    }
}

tasks.named("build") {
    dependsOn("shadowJar")
}

// Bequemer Hinweis, falls jemand ohne -Pvariant baut.
gradle.taskGraph.whenReady {
    logger.lifecycle("HugoAFKClient-Build: variant=$variant  (MCProtocolLib $mcProtocolLibVersion, Via=${if (viaEnabled) "an" else "aus"})")
}
