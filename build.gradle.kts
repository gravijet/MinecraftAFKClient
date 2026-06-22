plugins {
    application
    id("com.gradleup.shadow") version "8.3.5"
}

group = "com.hugoafk"
version = "1.0.0"

val mcProtocolLibVersion: String by project
val minecraftAuthVersion: String by project
val adventureVersion: String by project
val jlineVersion: String by project
val gsonVersion: String by project
val slf4jVersion: String by project

repositories {
    mavenCentral()
    // MCProtocolLib (GeyserMC)
    maven("https://repo.opencollab.dev/maven-releases/")
    maven("https://repo.opencollab.dev/maven-snapshots/")
    // MinecraftAuth (RaphiMC) + Lenni0451 commons
    maven("https://maven.lenni0451.net/releases")
    maven("https://maven.lenni0451.net/snapshots")
}

dependencies {
    implementation("org.geysermc.mcprotocollib:protocol:$mcProtocolLibVersion")
    implementation("net.raphimc:MinecraftAuth:$minecraftAuthVersion")

    // Chat-Komponenten farbig im Terminal rendern
    implementation(platform("net.kyori:adventure-bom:$adventureVersion"))
    implementation("net.kyori:adventure-text-serializer-ansi")
    implementation("net.kyori:adventure-text-serializer-plain")

    // Eingabezeile getrennt vom scrollenden Output
    implementation("org.jline:jline:$jlineVersion")

    implementation("com.google.code.gson:gson:$gsonVersion")
    implementation("org.slf4j:slf4j-simple:$slf4jVersion")
}

java {
    toolchain {
        languageVersion.set(JavaLanguageVersion.of(21))
    }
}

application {
    mainClass.set("com.hugoafk.Main")
}

tasks.shadowJar {
    archiveBaseName.set("hugoafkclient")
    archiveClassifier.set("")
    archiveVersion.set("")
    mergeServiceFiles()
}

tasks.named("build") {
    dependsOn("shadowJar")
}
