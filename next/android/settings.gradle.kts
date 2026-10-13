// The product identity lives in ../identity.toml; read it here instead of repeating it.
fun identity(key: String): String {
    val line = file("../identity.toml").readLines().first { it.trim().startsWith("$key =") }
    return line.substringAfter("=").substringBefore("#").trim().trim('"')
}

rootProject.name = "${identity("name")}-android"

pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositories {
        mavenCentral()
    }
}

include(":contract")
