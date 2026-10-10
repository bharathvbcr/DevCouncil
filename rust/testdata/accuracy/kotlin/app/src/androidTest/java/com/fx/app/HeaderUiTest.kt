package com.fx.app

import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import org.junit.Rule
import org.junit.Test

// K21 Compose UI test
class HeaderUiTest {
    @get:Rule
    val rule = createComposeRule()

    @Test
    fun showsTitle() {
        rule.setContent { Header("Hello") }                 // EXPECT showsTitle -> Header
        rule.onNodeWithText("Hello").assertExists()
    }
}
