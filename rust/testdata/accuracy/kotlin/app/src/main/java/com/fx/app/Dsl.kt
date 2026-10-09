package com.fx.app

// K12 lambda with receiver / DSL builder
class Page {
    val sections = mutableListOf<String>()
    fun section(name: String) { sections += name }
}

fun buildPage(block: Page.() -> Unit): Page = Page().apply(block)   // EXPECT buildPage -> Page (ctor)
