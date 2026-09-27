' termwall: backend statystyk dla tapety (127.0.0.1:9002), w tle bez okna.
Set sh = CreateObject("WScript.Shell")
sh.CurrentDirectory = "E:\Pliki\Projects\termwall"
sh.Run """C:\Users\halas\AppData\Local\Python\bin\pythonw.exe"" ""E:\Pliki\Projects\termwall\termwall_api.py""", 0, False
