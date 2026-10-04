/* Public, deliberately synthetic data only. Never run against another app.
 * Build: x86_64-w64-mingw32-gcc -std=c11 -Wall -Wextra -Werror -municode
 *   -DUNICODE -D_UNICODE owned_fixture.c -o bin/owned-fixture-x64.exe
 *   -lcomctl32 -luser32 -lgdi32 -lshell32
 */
#define WIN32_LEAN_AND_MEAN
#define _WIN32_WINNT 0x0601
#define _WIN32_IE 0x0600
#include <windows.h>
#include <commctrl.h>
#include <richedit.h>
#include <shellapi.h>
#include <stdio.h>
#include <stdint.h>
#include <wchar.h>

#define FIXTURE_CLASS L"CoralSpyOwnedFixtureWindow"
#define WM_FIXTURE_STALL (WM_APP + 41)
#define WM_FIXTURE_MENU (WM_APP + 42)
#define WM_FIXTURE_END_MENU (WM_APP + 43)
#define WM_FIXTURE_SLOW_RTF (WM_APP + 44)
#define TIMER_MENU 1
#define ID_MENU_BUTTON 101
#define ID_STALL_BUTTON 102
#define ID_PUBLIC_ACTION 201

static HWND main_window, rich, password_edit, password_rich, list, tree, wrong_class, status_text;
static HMENU app_menu, popup_menu;
static HMODULE rich_library;
static wchar_t manifest_path[32768];
static char run_nonce[65];
static BOOL stalling, menu_active, slow_rtf, streaming;
static HWND popup_window;
static ULONGLONG menu_deadline;

static unsigned long long window_number(HWND window) {
    return (unsigned long long)(uintptr_t)window;
}

static BOOL CALLBACK find_own_popup(HWND window, LPARAM unused) {
    wchar_t name[64];
    DWORD pid = 0;
    (void)unused;
    GetWindowThreadProcessId(window, &pid);
    if (pid == GetCurrentProcessId() && IsWindowVisible(window) &&
        GetClassNameW(window, name, 64) && wcscmp(name, L"#32768") == 0) {
        popup_window = window;
        return FALSE;
    }
    return TRUE;
}

static void publish_manifest(BOOL echo) {
    char json[4096];
    int length = snprintf(json, sizeof(json),
        "{\"fixture\":\"coralspy-owned-public-fixture-v1\",\"nonce\":\"%s\","
        "\"pid\":%lu,\"tid\":%lu,\"pointer_bits\":%u,"
        "\"main_hwnd\":%llu,\"rich_edit_hwnd\":%llu,"
        "\"password_edit_hwnd\":%llu,\"password_rich_edit_hwnd\":%llu,"
        "\"list_view_hwnd\":%llu,\"tree_view_hwnd\":%llu,"
        "\"wrong_class_hwnd\":%llu,\"menu_owner_hwnd\":%llu,"
        "\"popup_hwnd\":%llu,\"app_menu\":%llu,\"popup_menu\":%llu,"
        "\"stalling\":%s,\"menu_active\":%s,\"streaming\":%s,\"slow_rtf\":%s,"
        "\"messages\":{\"stall\":%u,\"open_menu\":%u,\"close_menu\":%u,\"slow_rtf\":%u}}\n",
        run_nonce, (unsigned long)GetCurrentProcessId(), (unsigned long)GetCurrentThreadId(),
        (unsigned int)(8 * sizeof(void *)), window_number(main_window), window_number(rich),
        window_number(password_edit), window_number(password_rich), window_number(list),
        window_number(tree), window_number(wrong_class), window_number(main_window),
        window_number(popup_window), (unsigned long long)(uintptr_t)app_menu,
        (unsigned long long)(uintptr_t)popup_menu, stalling ? "true" : "false",
        menu_active ? "true" : "false", streaming ? "true" : "false", slow_rtf ? "true" : "false",
        WM_FIXTURE_STALL, WM_FIXTURE_MENU, WM_FIXTURE_END_MENU, WM_FIXTURE_SLOW_RTF);
    if (length <= 0 || (size_t)length >= sizeof(json)) return;
    if (echo) { fwrite(json, 1, (size_t)length, stdout); fflush(stdout); }
    if (manifest_path[0]) {
        wchar_t temp_path[32768];
        FILE *file;
        unsigned int attempt;
        if (swprintf(temp_path, 32768, L"%ls.tmp", manifest_path) < 0) return;
        file = _wfopen(temp_path, L"wb");
        if (!file) return;
        if (fwrite(json, 1, (size_t)length, file) != (size_t)length) {
            fclose(file); DeleteFileW(temp_path); return;
        }
        fclose(file);
        for (attempt = 0; attempt < 25; ++attempt) {
            if (MoveFileExW(temp_path, manifest_path, MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)) return;
            /* A harness reader may briefly hold the old file without delete sharing. */
            Sleep(2);
        }
        fputs("Cannot publish fixture state after a bounded file-sharing retry.\n", stderr);
    }
}

static LRESULT CALLBACK rich_subclass(HWND window, UINT message, WPARAM wparam,
                                     LPARAM lparam, UINT_PTR subclass_id, DWORD_PTR data) {
    LRESULT result;
    (void)subclass_id; (void)data;
    if (message == EM_STREAMOUT && slow_rtf) {
        streaming = TRUE; publish_manifest(FALSE);
        Sleep(2500); /* Fixed delay only, for in-flight broker cancellation tests. */
        result = DefSubclassProc(window, message, wparam, lparam);
        streaming = FALSE; publish_manifest(FALSE);
        return result;
    }
    return DefSubclassProc(window, message, wparam, lparam);
}

static HWND child(const wchar_t *cls, const wchar_t *text, DWORD style,
                  int x, int y, int width, int height, int id) {
    return CreateWindowExW(0, cls, text, WS_CHILD | WS_VISIBLE | style,
        x, y, width, height, main_window, (HMENU)(INT_PTR)id, GetModuleHandleW(NULL), NULL);
}

static void add_column(int index, wchar_t *title, int width) {
    LVCOLUMNW col;
    ZeroMemory(&col, sizeof(col));
    col.mask = LVCF_TEXT | LVCF_WIDTH | LVCF_SUBITEM;
    col.pszText = title; col.cx = width; col.iSubItem = index;
    SendMessageW(list, LVM_INSERTCOLUMNW, index, (LPARAM)&col);
}

static void add_row(int index, wchar_t *name, wchar_t *value, wchar_t *note) {
    LVITEMW item;
    ZeroMemory(&item, sizeof(item));
    item.mask = LVIF_TEXT; item.iItem = index; item.pszText = name;
    SendMessageW(list, LVM_INSERTITEMW, 0, (LPARAM)&item);
    item.iSubItem = 1; item.pszText = value;
    SendMessageW(list, LVM_SETITEMTEXTW, index, (LPARAM)&item);
    item.iSubItem = 2; item.pszText = note;
    SendMessageW(list, LVM_SETITEMTEXTW, index, (LPARAM)&item);
}

static HTREEITEM add_node(HTREEITEM parent, wchar_t *label) {
    TVINSERTSTRUCTW item;
    ZeroMemory(&item, sizeof(item));
    item.hParent = parent; item.hInsertAfter = TVI_LAST;
    item.item.mask = TVIF_TEXT; item.item.pszText = label;
    return (HTREEITEM)SendMessageW(tree, TVM_INSERTITEMW, 0, (LPARAM)&item);
}

static BOOL make_controls(void) {
    CHARFORMAT2W format;
    CHARRANGE selection = {0, 14};
    HTREEITEM root, child_node;
    child(L"STATIC", L"Owned test fixture: all text below is public sample data. No other app is inspected.",
        0, 12, 10, 830, 25, 0);
    child(L"STATIC", L"Formatted Unicode RichEdit", 0, 12, 40, 420, 22, 0);
    rich = child(MSFTEDIT_CLASS, L"", WS_BORDER | ES_MULTILINE | ES_READONLY | WS_VSCROLL,
        12, 65, 510, 140, 0);
    if (!rich) return FALSE;
    if (!SetWindowSubclass(rich, rich_subclass, 1, 0)) return FALSE;
    SetWindowTextW(rich, L"Public fixture \u2014 caf\u00e9 \u65e5\u672c\u8a9e \U0001f600\r\nBold blue fixture text.\r\nSecond paragraph.");
    SendMessageW(rich, EM_EXSETSEL, 0, (LPARAM)&selection);
    ZeroMemory(&format, sizeof(format)); format.cbSize = sizeof(format);
    format.dwMask = CFM_BOLD | CFM_COLOR; format.dwEffects = CFE_BOLD;
    format.crTextColor = RGB(0, 50, 180);
    SendMessageW(rich, EM_SETCHARFORMAT, SCF_SELECTION, (LPARAM)&format);
    selection.cpMin = selection.cpMax = 0;
    SendMessageW(rich, EM_EXSETSEL, 0, (LPARAM)&selection);
    child(L"STATIC", L"Password controls: public rejection-test text", 0, 540, 40, 320, 24, 0);
    password_edit = child(L"EDIT", L"PUBLIC-FIXTURE-NOT-A-SECRET", WS_BORDER | ES_PASSWORD,
        540, 70, 310, 30, 0);
    password_rich = child(MSFTEDIT_CLASS, L"", WS_BORDER | ES_PASSWORD,
        540, 110, 310, 30, 0);
    SetWindowTextW(password_rich, L"PUBLIC-RICH-FIXTURE-NOT-A-SECRET");
    SendMessageW(password_rich, EM_SETPASSWORDCHAR, L'*', 0);
    wrong_class = child(L"STATIC", L"Wrong-class rejection target (STATIC)", WS_BORDER,
        540, 155, 310, 40, 0);
    child(L"STATIC", L"Unicode ListView: 3 columns, 3 rows", 0, 12, 218, 510, 24, 0);
    list = child(WC_LISTVIEWW, L"", WS_BORDER | LVS_REPORT | LVS_SINGLESEL,
        12, 245, 510, 165, 0);
    ListView_SetExtendedListViewStyle(list, LVS_EX_FULLROWSELECT | LVS_EX_GRIDLINES);
    add_column(0, L"Name 名", 170); add_column(1, L"", 150); add_column(2, L"Note\t备注", 160);
    add_row(0, L"Alpha", L"caf\u00e9", L"public row 1");
    add_row(1, L"\u65e5\u672c\u8a9e", L"\u03b2eta", L"public row 2");
    add_row(2, L"Emoji \U0001f600", L"42", L"public row 3");
    child(L"STATIC", L"Known TreeView hierarchy", 0, 540, 218, 310, 24, 0);
    tree = child(WC_TREEVIEWW, L"", WS_BORDER | TVS_HASBUTTONS | TVS_HASLINES | TVS_LINESATROOT,
        540, 245, 310, 165, 0);
    root = add_node(TVI_ROOT, L"Public root");
    child_node = add_node(root, L"Branch caf\u00e9");
    add_node(child_node, L"Leaf \u65e5\u672c\u8a9e");
    add_node(root, L"Branch \U0001f600");
    TreeView_Expand(tree, root, TVE_EXPAND); TreeView_Expand(tree, child_node, TVE_EXPAND);
    add_node(TVI_ROOT, L"Second root");
    child(L"BUTTON", L"Open public fixture popup (5 seconds)", BS_PUSHBUTTON,
        12, 425, 310, 32, ID_MENU_BUTTON);
    child(L"BUTTON", L"Pause own UI for 2 seconds", BS_PUSHBUTTON,
        335, 425, 255, 32, ID_STALL_BUTTON);
    status_text = child(L"STATIC", L"Ready. Close this window to stop the fixture.", 0,
        12, 475, 830, 24, 0);
    return password_edit && password_rich && list && tree && wrong_class && status_text;
}

static DWORD CALLBACK write_golden_rtf(DWORD_PTR cookie, LPBYTE bytes, LONG length, LONG *written) {
    FILE *file = (FILE *)cookie;
    if (length < 0) return 1;
    *written = (LONG)fwrite(bytes, 1, (size_t)length, file);
    return *written == length ? 0 : 1;
}

static BOOL save_golden_rtf(void) {
    wchar_t path[32768];
    FILE *file;
    EDITSTREAM stream;
    BOOL ok;
    if (swprintf(path, 32768, L"%ls.expected.rtf", manifest_path) < 0) return FALSE;
    file = _wfopen(path, L"wb");
    if (!file) return FALSE;
    ZeroMemory(&stream, sizeof(stream));
    stream.dwCookie = (DWORD_PTR)file;
    stream.pfnCallback = write_golden_rtf;
    SendMessageW(rich, EM_STREAMOUT, SF_RTF, (LPARAM)&stream);
    ok = stream.dwError == 0 && !ferror(file);
    if (fclose(file) != 0) ok = FALSE;
    return ok;
}

static void show_popup(void) {
    RECT rect;
    if (menu_active) return;
    GetWindowRect(main_window, &rect);
    menu_active = TRUE; popup_window = NULL;
    menu_deadline = GetTickCount64() + 5000;
    SetTimer(main_window, TIMER_MENU, 50, NULL);
    publish_manifest(FALSE);
    TrackPopupMenuEx(popup_menu, TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RIGHTBUTTON,
        rect.left + 40, rect.top + 90, main_window, NULL);
    KillTimer(main_window, TIMER_MENU);
    menu_active = FALSE; popup_window = NULL;
    publish_manifest(FALSE);
}

static LRESULT CALLBACK window_proc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
    switch (message) {
    case WM_COMMAND:
        if (LOWORD(wparam) == ID_MENU_BUTTON) show_popup();
        else if (LOWORD(wparam) == ID_STALL_BUTTON) PostMessageW(window, WM_FIXTURE_STALL, 2000, 0);
        else if (LOWORD(wparam) == ID_PUBLIC_ACTION) SetWindowTextW(status_text, L"Public fixture action selected.");
        return 0;
    case WM_FIXTURE_STALL: {
        DWORD duration = (DWORD)wparam;
        if (duration > 5000) duration = 5000;
        if (!duration) duration = 2000;
        stalling = TRUE; SetWindowTextW(status_text, L"Synthetic UI stall in this fixture only.");
        publish_manifest(FALSE);
        Sleep(duration);
        stalling = FALSE; SetWindowTextW(status_text, L"Ready after synthetic stall.");
        publish_manifest(FALSE);
        return 0;
    }
    case WM_FIXTURE_MENU: show_popup(); return 0;
    case WM_FIXTURE_END_MENU: EndMenu(); return 0;
    case WM_FIXTURE_SLOW_RTF: slow_rtf = wparam != 0; publish_manifest(FALSE); return 0;
    case WM_TIMER:
        if (wparam == TIMER_MENU) {
            popup_window = NULL;
            EnumThreadWindows(GetCurrentThreadId(), find_own_popup, 0);
            publish_manifest(FALSE);
            if (GetTickCount64() >= menu_deadline) EndMenu();
        }
        return 0;
    case WM_DESTROY: PostQuitMessage(0); return 0;
    default: return DefWindowProcW(window, message, wparam, lparam);
    }
}

int wmain(int argc, wchar_t **argv) {
    WNDCLASSW wc;
    MSG message;
    INITCOMMONCONTROLSEX controls = {sizeof(controls), ICC_LISTVIEW_CLASSES | ICC_TREEVIEW_CLASSES};
    size_t i;
    if (argc != 3 || wcslen(argv[1]) > 32000 || wcslen(argv[2]) > 64 || !argv[2][0]) {
        fwprintf(stderr, L"Usage: owned-fixture-{x64,x86}.exe <manifest.json> <ASCII run nonce>\n");
        return 2;
    }
    wcscpy(manifest_path, argv[1]);
    for (i = 0; argv[2][i]; ++i) {
        wchar_t ch = argv[2][i];
        if (!((ch >= L'0' && ch <= L'9') || (ch >= L'a' && ch <= L'z') ||
              (ch >= L'A' && ch <= L'Z') || ch == L'-')) return 2;
        run_nonce[i] = (char)ch;
    }
    InitCommonControlsEx(&controls);
    rich_library = LoadLibraryW(L"Msftedit.dll");
    if (!rich_library) { fputs("Cannot load the Windows RichEdit library.\n", stderr); return 3; }
    ZeroMemory(&wc, sizeof(wc));
    wc.lpfnWndProc = window_proc; wc.hInstance = GetModuleHandleW(NULL);
    wc.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1); wc.hCursor = LoadCursorW(NULL, IDC_ARROW);
    wc.lpszClassName = FIXTURE_CLASS;
    if (!RegisterClassW(&wc)) return 3;
    app_menu = CreateMenu(); popup_menu = CreatePopupMenu();
    AppendMenuW(popup_menu, MF_STRING, ID_PUBLIC_ACTION, L"Public action caf\u00e9");
    AppendMenuW(popup_menu, MF_STRING | MF_GRAYED, 202, L"Disabled public item");
    AppendMenuW(popup_menu, MF_SEPARATOR, 0, NULL);
    AppendMenuW(popup_menu, MF_STRING, 203, L"Unicode \u65e5\u672c\u8a9e \U0001f600");
    AppendMenuW(app_menu, MF_POPUP, (UINT_PTR)popup_menu, L"Public &fixture menu");
    main_window = CreateWindowExW(0, FIXTURE_CLASS,
        L"CoralSpy OWNED TEST FIXTURE - public synthetic data only",
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
        CW_USEDEFAULT, CW_USEDEFAULT, 885, 585, NULL, app_menu, wc.hInstance, NULL);
    if (!main_window || !make_controls()) {
        fputs("Cannot create a required fixture control.\n", stderr); return 3;
    }
    ShowWindow(main_window, SW_SHOW); UpdateWindow(main_window);
    if (!save_golden_rtf()) {
        fputs("Cannot write public golden RTF fixture bytes.\n", stderr); return 3;
    }
    publish_manifest(TRUE);
    while (GetMessageW(&message, NULL, 0, 0) > 0) {
        TranslateMessage(&message); DispatchMessageW(&message);
    }
    DestroyMenu(app_menu); FreeLibrary(rich_library);
    return 0;
}
