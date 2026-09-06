program Probe;

uses Helper;

type
  TWidget = class(TBaseWidget)
    function Render: string;
  end;

function TWidget.Render: string;
begin
  Result := Help(Name);
end;

begin
  WriteLn('x');
end.
