import org.gradle.api.tasks.testing.logging.TestExceptionFormat

plugins {
    alias(libs.plugins.kotlin.jvm)
    alias(libs.plugins.kotlin.serialization)
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    api(project(":contract"))
    implementation(libs.zxing.core)
    testImplementation(libs.kotlin.test)
}

tasks.test {
    useJUnitPlatform()
    systemProperty("contract.dir", rootDir.resolve("../contract").canonicalPath)
    inputs.dir(rootDir.resolve("../contract")).withPropertyName("contract")
    testLogging {
        events("failed")
        exceptionFormat = TestExceptionFormat.FULL
    }
}
