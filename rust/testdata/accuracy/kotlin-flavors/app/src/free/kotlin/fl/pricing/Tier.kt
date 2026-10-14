package fl.pricing

// The `free` product flavor's Tier. The `paid` flavor declares the same class
// in the same package; a variant compiles exactly one of them.
object Tier {
    fun label(): String = "free"
}
