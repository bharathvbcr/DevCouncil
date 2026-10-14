package fl

import fl.core.Money
import fl.pricing.Tier

fun showFree(): String = Tier.label() + Money.cents(1)
