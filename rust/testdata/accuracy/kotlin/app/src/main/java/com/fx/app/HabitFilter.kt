package com.fx.app

// K32 (added for the dead-policy card, second pass): the corpus shape of K30.
// A private *extension* function reached only as `template::matchesFilterTerm`
// on an untyped lambda parameter — ChronosFlow's HabitFormSheet.kt. The bare
// member name never matches the extension's key `FilterTemplate.matchesFilterTerm`,
// so without the reference's receiver the namesake veto never saw it.
internal data class FilterTemplate(val name: String)

internal fun filterTemplates(templates: List<FilterTemplate>, query: String): List<FilterTemplate> {
    val terms = query.split(" ")
    return templates.filter { template ->
        terms.all(template::matchesFilterTerm)
    }
}

private fun FilterTemplate.matchesFilterTerm(term: String): Boolean = name.contains(term)
