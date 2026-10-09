package com.fx.app

import androidx.compose.foundation.layout.Column
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.hilt.navigation.compose.hiltViewModel
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController

// K01 Compose: composition calls, trailing lambdas, content slots
@Composable
fun HomeScreen(vm: MainViewModel = hiltViewModel()) {
    val state by vm.state.collectAsState()                 // K02
    Column {
        Header(title = vm.describe(state))                 // EXPECT HomeScreen -> Header ; HomeScreen -> MainViewModel.describe
        Card(onClick = vm::refresh) {                      // EXPECT HomeScreen -> Card ; ref MainViewModel.refresh (K27)
            Text("tap")
        }
    }
}

@Composable
fun Header(title: String) { Text(title) }

@Composable
fun Card(onClick: () -> Unit, content: @Composable () -> Unit) {
    content()                                             // slot invocation (param), no project edge
    onClick()
}

@Composable
fun UnusedBadge() { Text("never shown") }                  // EXPECT dead (truly unused composable)

// K18 navigation graph
@Composable
fun AppNavGraph() {
    val nav = rememberNavController()
    NavHost(navController = nav, startDestination = "home") {
        composable("home") { HomeScreen() }                // EXPECT AppNavGraph -> HomeScreen
        composable("settings") { SettingsScreen(onBack = { nav.popBackStack() }) }  // EXPECT AppNavGraph -> SettingsScreen
    }
}

@Composable
fun SettingsScreen(onBack: () -> Unit) { Header("settings") }  // EXPECT SettingsScreen -> Header
