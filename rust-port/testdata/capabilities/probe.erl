-module(probe).
-behaviour(gen_server).
-export([render/1, main/0]).

-import(helper, [help/1]).
-include("records.hrl").

render(Name) ->
    helper:help(Name).

main() ->
    render("x").
