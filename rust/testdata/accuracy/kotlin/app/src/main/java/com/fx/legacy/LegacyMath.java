package com.fx.legacy;

// K23 Java interop: Java static called from Kotlin, Java calling Kotlin object.
public final class LegacyMath {
    public static int twice(int x) { return x * 2; }
    public static void callKotlin() { com.fx.app.Registry.INSTANCE.register("java"); }   // EXPECT callKotlin -> Registry.register
}
