library(helper)
source("helpers.R")

render <- function(name) {
  help_fn(name)
}

main <- function() {
  render("x")
}
