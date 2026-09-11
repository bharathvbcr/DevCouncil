{ lib }:
let greet = name: "hello ${name}";
in { message = greet "fixture"; }
