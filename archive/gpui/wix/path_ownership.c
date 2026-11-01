/* Keep an existing, externally managed PATH entry outside MSI ownership.
 * This action only reads registry values and changes MSI's component plan.
 * Windows Installer remains responsible for transactional PATH append/removal.
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <msiquery.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

static wchar_t *property(MSIHANDLE install, const wchar_t *name) {
    DWORD length = 0;
    wchar_t empty[1] = {0};
    UINT result = MsiGetPropertyW(install, name, empty, &length);
    if (result != ERROR_MORE_DATA && result != ERROR_SUCCESS) return NULL;
    wchar_t *value = calloc((size_t)length + 1, sizeof(wchar_t));
    if (!value) return NULL;
    ++length;
    if (MsiGetPropertyW(install, name, value, &length) != ERROR_SUCCESS) {
        free(value); return NULL;
    }
    return value;
}

static wchar_t *registry_string(HKEY key, const wchar_t *name) {
    DWORD bytes = 0, type = 0;
    LONG result = RegQueryValueExW(key, name, NULL, &type, NULL, &bytes);
    if (result == ERROR_FILE_NOT_FOUND) return calloc(1, sizeof(wchar_t));
    if (result != ERROR_SUCCESS || (type != REG_SZ && type != REG_EXPAND_SZ)) return NULL;
    wchar_t *value = calloc((size_t)bytes + sizeof(wchar_t), 1);
    if (!value) return NULL;
    if (RegQueryValueExW(key, name, NULL, &type, (BYTE *)value, &bytes) != ERROR_SUCCESS) {
        free(value); return NULL;
    }
    return value;
}

static wchar_t *normalized_path(const wchar_t *input) {
    DWORD capacity = ExpandEnvironmentStringsW(input, NULL, 0);
    if (!capacity) return NULL;
    wchar_t *value = calloc(capacity, sizeof(wchar_t));
    if (!value) return NULL;
    DWORD written = ExpandEnvironmentStringsW(input, value, capacity);
    if (!written || written > capacity) { free(value); return NULL; }
    size_t length = wcslen(value);
    if (length >= 2 && value[0] == L'"' && value[length - 1] == L'"') {
        memmove(value, value + 1, (length - 2) * sizeof(wchar_t));
        value[length -= 2] = 0;
    }
    while (length > 3 && (value[length - 1] == L'\\' || value[length - 1] == L'/')) value[--length] = 0;
    for (size_t i = 0; i < length; ++i) if (value[i] == L'/') value[i] = L'\\';
    return value;
}

__declspec(dllexport) UINT __stdcall PreserveForeignPath(MSIHANDLE install) {
    UINT result = ERROR_INSTALL_FAILURE;
    INSTALLSTATE installed, action;
    HKEY environment = NULL, owner = NULL;
    wchar_t *target = NULL, *path = NULL, *registry = NULL, *previous = NULL;
    wchar_t *normalized_target = NULL, *normalized_previous = NULL;
    if (MsiGetComponentStateW(install, L"CommandLinePath", &installed, &action) != ERROR_SUCCESS) goto cleanup;
    if (action != INSTALLSTATE_LOCAL) { result = ERROR_SUCCESS; goto cleanup; }
    target = property(install, L"APPLICATIONFOLDER");
    registry = property(install, L"XenTermInstallerRegistryKey");
    if (!target || !*target || !registry || !*registry) goto cleanup;
    normalized_target = normalized_path(target);
    if (!normalized_target) goto cleanup;

    /* Repair knows ownership from MSI itself. Major upgrades use the old
       package's own marker and path, before RemoveExistingProducts runs. */
    if (installed == INSTALLSTATE_LOCAL) { result = ERROR_SUCCESS; goto cleanup; }
    LONG opened = RegOpenKeyExW(HKEY_LOCAL_MACHINE, registry, 0, KEY_READ | KEY_WOW64_64KEY, &owner);
    if (opened != ERROR_SUCCESS && opened != ERROR_FILE_NOT_FOUND) goto cleanup;
    if (owner) {
        DWORD owned = 0, bytes = sizeof(owned), type = 0;
        LONG read = RegQueryValueExW(owner, L"commandLinePath", NULL, &type, (BYTE *)&owned, &bytes);
        if (read != ERROR_SUCCESS && read != ERROR_FILE_NOT_FOUND) goto cleanup;
        if (read == ERROR_SUCCESS && type == REG_DWORD && owned == 1) {
            previous = registry_string(owner, L"InstallDir");
            if (!previous) goto cleanup;
            normalized_previous = normalized_path(previous);
            if (!normalized_previous) goto cleanup;
            if (CompareStringOrdinal(normalized_target, -1, normalized_previous, -1, TRUE) == CSTR_EQUAL) { result = ERROR_SUCCESS; goto cleanup; }
        }
    }
    if (RegOpenKeyExW(HKEY_LOCAL_MACHINE,
        L"SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment", 0,
        KEY_READ | KEY_WOW64_64KEY, &environment) != ERROR_SUCCESS) goto cleanup;
    path = registry_string(environment, L"Path");
    if (!path) goto cleanup;
    for (wchar_t *entry = path; entry;) {
        wchar_t *next = wcschr(entry, L';');
        if (next) *next++ = 0;
        wchar_t *normalized_entry = normalized_path(entry);
        if (!normalized_entry) goto cleanup;
        BOOL match = CompareStringOrdinal(normalized_entry, -1, normalized_target, -1, TRUE) == CSTR_EQUAL;
        free(normalized_entry);
        if (match) {
            result = MsiSetComponentStateW(install, L"CommandLinePath", INSTALLSTATE_ABSENT);
            goto cleanup;
        }
        entry = next;
    }
    result = ERROR_SUCCESS;
cleanup:
    if (owner) RegCloseKey(owner);
    if (environment) RegCloseKey(environment);
    free(target); free(path); free(registry); free(previous);
    free(normalized_target); free(normalized_previous);
    return result;
}
