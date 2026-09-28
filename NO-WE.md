# termwall bez Wallpaper Engine

Windows nie ma już natywnych tapet HTML (Active Desktop zniknął w Vista). Każdy
program z "tapetą web" (WE, Lively) robi to samo: tworzy okno z przeglądarką i
**wkłada je pod ikony pulpitu**. Da się to zrobić samemu. termwall prawie w ogóle
nie zależy od WE: poza `wallpaperRegisterMediaPropertiesListener` (bez niego linia
z utworem jest po prostu pusta) to zwykła strona, która co sekundę robi `fetch`
do API.

## Opcje

| Opcja | Na żywo | Nakład | Uwagi |
|---|---|---|---|
| **A. Własny host: WebView2 + WorkerW** | tak (1 s) | ~150 linii | robi to samo co WE/Lively, ale każdy edge case jest na tobie |
| **B. Zrzut ekranu → zwykła tapeta** | nie (co 15–60 s) | ~40 linii, `winwall.py` już jest | zero okien i hacków, ale bez animacji, a zegar skacze |
| C. Lively | tak | 0 | open source, darmowy; odpada tylko jeśli nie chcesz niczego dodatkowego |
| D. Rainmeter | tak | przepisanie | to już nie jest HTML |

Polecam **A**, jeśli chcesz, żeby wyglądało tak samo jak teraz, a **B**, jeśli
wystarczy odświeżanie co kilkadziesiąt sekund i ma być niezawodnie.

---

## A. Własny host (WorkerW)

### Mechanizm

1. `FindWindow("Progman")` i wysłanie do niego wiadomości **`0x052C`**
   (`SendMessageTimeout`, wParam/lParam = 0). Explorer tworzy wtedy dodatkowe okno
   `WorkerW`, które leży **między** tapetą a ikonami (`SHELLDLL_DefView`). To jest
   nieudokumentowane, ale korzystają z tego WE, Lively i weebp od lat.
2. Znalezienie tego `WorkerW`:
   - **Win10 i Win11 przed 24H2:** `EnumWindows`, znaleźć top-level `WorkerW`, który ma
     dziecko `SHELLDLL_DefView`, a potem wziąć **następne** rodzeństwo
     `FindWindowEx(None, ten_workerw, "WorkerW", None)`.
   - **Win11 24H2+:** hierarchia się zmieniła, `WorkerW` jest teraz **dzieckiem
     `Progman`** (`FindWindowEx(progman, None, "WorkerW", None)`), a ikony siedzą
     obok niego w `Progman`. *Wiem to z tego, jak łatano Lively/weebp pod 24H2, nie
     z dokumentacji MS. Sprawdź na swoim buildzie w Spy++ / System Informer.*
   - Najlepiej obsłużyć oba przypadki: najpierw dziecko Progman, a jeśli go nie ma, stara metoda.
3. Stworzenie okna bez ramki z WebView2 i załadowanie `index.html`.
4. `SetParent(hwnd_okna, workerw)`, a potem `SetWindowPos` na prostokąt monitora
   **we współrzędnych WorkerW**. WorkerW obejmuje cały virtual screen, więc monitor
   na lewo od głównego ma ujemne X, a ty musisz odjąć origin
   (`GetSystemMetrics(SM_XVIRTUALSCREEN/SM_YVIRTUALSCREEN)`).

### Czym hostować WebView2

- **Python + `pywebview`** (na Windows używa WebView2 przez pythonnet). Masz już
  Pythona i ctypes w `winwall.py`, więc to najkrótsza droga. HWND weźmiesz z
  `window.native.Handle` (WinForms) po evencie `shown`.
- **C# / WinForms + `Microsoft.Web.WebView2`**: jeden `.exe`, najstabilniejsze,
  pełna kontrola nad `CoreWebView2` (np. `TrySuspendAsync` przy pauzie).
- AutoHotkey v2 + WebView2.ahk: działa, ale debugowanie jest gorsze.

Szkic (Python, idea zamiast gotowca):

```text
progman = FindWindowW("Progman", None)
SendMessageTimeoutW(progman, 0x052C, 0, 0, SMTO_NORMAL, 1000, byref(res))
workerw = FindWindowExW(progman, None, "WorkerW", None)        # 24H2+
if not workerw:                                                # starsze
    EnumWindows(cb)  # cb: jeśli FindWindowExW(hwnd, None, "SHELLDLL_DefView") →
                     #     workerw = FindWindowExW(None, hwnd, "WorkerW", None)
webview.create_window(url, frameless=True, ...)
on shown: SetParent(hwnd, workerw); SetWindowPos(hwnd, rect_monitora - origin_virtual_screen)
```

### Co WE robił za ciebie (i co musisz dopisać sam)

- **Restart explorer.exe** niszczy WorkerW i twoje okno znika. Trzeba nasłuchiwać
  `RegisterWindowMessage("TaskbarCreated")` i podpiąć się od nowa.
- **Pauza przy fullscreen/grze.** Bez niej WebView2 renderuje cały czas. Sprawdzaj
  `SHQueryUserNotificationState` albo porównuj okno na pierwszym planie z rozmiarem
  monitora; wtedy `TrySuspendAsync` (C#) albo ukrycie okna. termwall i tak robi mało
  (1 fetch/s, bez canvas), więc to optymalizacja, a nie konieczność.
- **DPI:** proces musi być per-monitor DPI aware
  (`SetProcessDpiAwarenessContext(-4)`), inaczej rect monitora nie zgodzi się z pikselami.
- **Multi-monitor:** jedno okno na monitor albo jedno na cały virtual screen.
- **Autostart:** skrót w `shell:startup`, tak jak `termwall-api.vbs`. Host musi
  wystartować *po* API, ale strona i tak ponawia fetch, więc kolejność jest wybaczalna.
- **Kliknięcia:** okno w WorkerW jest pod ikonami, więc input do niego nie dochodzi.
  termwall nie potrzebuje inputu, więc nic tu nie trzeba robić.

### Strona: nic nie trzeba zmieniać, ale warto rozważyć jedno

Obecnie strona ładuje się z `file://` (origin `null`), a API odpowiada
`Access-Control-Allow-Origin: *` z tokenem z `token.js`. WebView2 z `file://` może
fetchować `http://127.0.0.1`, więc powinno działać bez zmian. Czystsza alternatywa
to serwowanie `index.html` i `token.js` przez samo API (`http://127.0.0.1:9002/`):
wtedy origin się zgadza, CORS `*` przestaje być potrzebny, a host ładuje zwykły URL.
To jednak zmiana w `termwall_api.py`, więc to decyzja na później.

---

## B. Zrzut → zwykła tapeta (bez żadnego okna)

1. Headless Chromium (Playwright, który masz) otwiera `index.html` w viewport
   1920×1080 (albo w rozdzielczości monitora) i robi `page.screenshot()` do PNG.
2. `winwall.py set plik.png INDEX` przez `IDesktopWallpaper::SetWallpaper`. Ten kod
   już istnieje w `rice`.
3. Pętla co N sekund, z przeglądarką trzymaną otwartą między zrzutami.

Ograniczenia: `SetWallpaper` przerysowuje cały pulpit (czasem z fade'em) i zapisuje
plik na dysk, więc co 1 s to zły pomysł. Realnie sensowne jest 15–60 s. Zegar i
sparkline'y przestają być "live", a wykresy historii dalej działają, bo strona
zbiera historię sama. Trzeba też pilnować, żeby zapisywać do dwóch plików na zmianę:
ta sama ścieżka bywa przez Windows cache'owana i tapeta się nie odświeża
(*z doświadczenia społeczności, niezweryfikowane na twoim buildzie*).
