package dev.alia.client

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialExpressiveTheme
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.expressiveLightColorScheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialExpressiveTheme(
                colorScheme = if (isSystemInDarkTheme()) {
                    darkColorScheme()
                } else {
                    expressiveLightColorScheme()
                },
            ) {
                Text("Alia")
            }
        }
    }
}
