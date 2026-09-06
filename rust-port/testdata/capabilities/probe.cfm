<cfinclude template="header.cfm">
<cfimport path="example.helper">
<cfscript>
    component Widget extends="BaseWidget" {
        function render() {
            return help(this.name);
        }
    }

    function main() {
        var w = new Widget();
        w.render();
    }
</cfscript>
