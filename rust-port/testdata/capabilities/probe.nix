{ pkgs ? import <nixpkgs> { } }:

let
  helper = import ./helper.nix { };
  render = name: helper.help name;
in
{
  value = render "x";
}
