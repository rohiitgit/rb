plugins {
    java
}

repositories {
    mavenCentral()
}

dependencies {
    implementation("com.google.code.gson:gson:2.13.1")
    implementation(libs.lang3)
    testImplementation("junit:junit:4.13.2")
}
