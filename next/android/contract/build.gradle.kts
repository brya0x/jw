import org.gradle.api.tasks.testing.logging.TestExceptionFormat

plugins {
    alias(libs.plugins.kotlin.jvm)
    alias(libs.plugins.kotlin.serialization)
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    api(libs.serialization.json)
    testImplementation(libs.kotlin.test)
    testImplementation(libs.schema.validator)
}

tasks.test {
    useJUnitPlatform()
    // The fixtures and the schema are shared with the Rust side.
    systemProperty("contract.dir", rootDir.resolve("../contract").canonicalPath)
    // Without this Gradle calls the task up to date when only a fixture changed.
    inputs.dir(rootDir.resolve("../contract")).withPropertyName("contract")
    testLogging {
        events("failed")
        exceptionFormat = TestExceptionFormat.FULL
    }
}
