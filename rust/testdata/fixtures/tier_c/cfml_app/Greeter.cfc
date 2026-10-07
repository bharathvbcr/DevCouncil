<cfcomponent displayname="Greeter">

    <cffunction name="greet" access="public" returntype="string">
        <cfargument name="name" type="string" required="true">
        <cfreturn "Hello, " & arguments.name & formatSuffix()>
    </cffunction>

    <cffunction name="formatSuffix" access="private" returntype="string">
        <cfreturn "!">
    </cffunction>

    <cffunction name="neverCalled" access="private" returntype="string">
        <cfreturn "nobody asks for this">
    </cffunction>

</cfcomponent>
