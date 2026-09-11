Imports Example.Helpers

Public Class Widget
    Inherits BaseWidget

    Public Function Render() As String
        Return Helper.Help(Me.Name)
    End Function

    Public Shared Sub Main()
        Dim w As New Widget()
        w.Render()
    End Sub
End Class
