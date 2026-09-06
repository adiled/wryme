' wryme desktop launcher - Windows
'
' Double-click target. Runs wryme-run.cmd invisibly so opening wryme feels
' like launching an app rather than a console window flashing up.

Option Explicit

Dim shell, fso, dir, cmd
Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")

dir = fso.GetParentFolderName(WScript.ScriptFullName)
cmd = """" & dir & "\wryme-run.cmd"""

shell.Run cmd, 0, False