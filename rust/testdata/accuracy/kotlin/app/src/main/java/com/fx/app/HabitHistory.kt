package com.fx.app

// K30 (added for the dead-policy card): a private member reached only through
// a bound callable reference `template::matchesHabitHistoryTerm`, the shape
// ChronosFlow's HabitFormSheet.kt uses. It must not be a confident dead row.
class HabitHistoryTemplate(private val name: String) {
    private fun matchesHabitHistoryTerm(term: String): Boolean = name.contains(term)

    fun matchesAll(terms: List<String>): Boolean {
        val template = this
        return terms.all(template::matchesHabitHistoryTerm)
    }
}

private fun isBlankTerm(term: String): Boolean = term.isBlank()

// A bare `::name` reference to a private top-level function.
fun dropBlank(terms: List<String>): List<String> = terms.filterNot(::isBlankTerm)

// K31: a private function nothing references — the fixture's confident true positive.
private fun unusedPrivateHabitHelper(): Int = 31
