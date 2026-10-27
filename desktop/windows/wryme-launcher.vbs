Option Explicit

Const HIDDEN_WINDOW = 0

Dim shell, fso, dir, cmd
Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")

dir = fso.GetParentFolderName(WScript.ScriptFullName)
cmd = """" & dir & "\wryme-run.cmd"""

shell.Run cmd, HIDDEN_WINDOW, False