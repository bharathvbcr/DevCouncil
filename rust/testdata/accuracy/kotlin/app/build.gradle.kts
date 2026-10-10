plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("com.google.dagger.hilt.android")
}
android {
    namespace = "com.fx.app"
    compileSdk = 35
    defaultConfig { minSdk = 26 }
}
// K22: a function declared in a build script, called once.
fun versionCodeFor(name: String): Int = name.length
val computed = versionCodeFor("1.0")
dependencies {
    implementation(project(":core"))
    testImplementation("junit:junit:4.13.2")
}
