' termwall: starts the stats API (127.0.0.1:9002) in the background, no console window.
' Portable: uses its own folder and pythonw from PATH. Put a shortcut to it in shell:startup.
Set fso = CreateObject("Scripting.FileSystemObject")
dir = fso.GetParentFolderName(WScript.ScriptFullName)
Set sh = CreateObject("WScript.Shell")
sh.CurrentDirectory = dir
sh.Run "pythonw """ & dir & "\termwall_api.py""", 0, False
