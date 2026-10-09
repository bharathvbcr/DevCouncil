// K22 Gradle Kotlin DSL: expect file indexed, no symbols invented, no false call edges into app code.
plugins {
    id("com.android.application") version "8.7.0" apply false
    id("org.jetbrains.kotlin.android") version "2.1.0" apply false
    id("com.google.dagger.hilt.android") version "2.52" apply false
}
