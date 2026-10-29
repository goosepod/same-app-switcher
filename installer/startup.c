// The application writes its Run value itself. Remove it after a successful
// uninstall only if it still points to this installation; portable copies and
// major upgrades must retain their own startup settings.
#include <windows.h>
#include <msi.h>
#include <msiquery.h>
#include <stdio.h>
#include <wchar.h>

__declspec(dllexport) UINT __stdcall RemoveStartupEntry(MSIHANDLE install)
{
    WCHAR directory[32768];
    DWORD directoryLength = ARRAYSIZE(directory);
    WCHAR expected[32768];
    WCHAR actual[32768];
    DWORD actualBytes = sizeof(actual);
    const WCHAR *runKey = L"Software\\Microsoft\\Windows\\CurrentVersion\\Run";

    if (MsiGetPropertyW(install, L"INSTALLFOLDER", directory, &directoryLength) != ERROR_SUCCESS || directoryLength == 0) {
        return ERROR_INSTALL_FAILURE;
    }
    if (swprintf_s(expected, ARRAYSIZE(expected), L"\"%lsSameAppSwitcher.exe\"", directory) < 0) {
        return ERROR_INSTALL_FAILURE;
    }
    LSTATUS result = RegGetValueW(HKEY_CURRENT_USER, runKey, L"SameAppSwitcher",
        RRF_RT_REG_SZ, NULL, actual, &actualBytes);
    if (result == ERROR_FILE_NOT_FOUND) {
        return ERROR_SUCCESS;
    }
    if (result != ERROR_SUCCESS) {
        return ERROR_INSTALL_FAILURE;
    }
    if (_wcsicmp(expected, actual) != 0) {
        return ERROR_SUCCESS;
    }
    HKEY key;
    result = RegOpenKeyExW(HKEY_CURRENT_USER, runKey, 0, KEY_SET_VALUE, &key);
    if (result != ERROR_SUCCESS) {
        return ERROR_INSTALL_FAILURE;
    }
    result = RegDeleteValueW(key, L"SameAppSwitcher");
    RegCloseKey(key);
    return (result == ERROR_SUCCESS || result == ERROR_FILE_NOT_FOUND) ? ERROR_SUCCESS : ERROR_INSTALL_FAILURE;
}
