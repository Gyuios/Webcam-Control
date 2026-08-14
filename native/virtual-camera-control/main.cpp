// CameraTuner virtual camera lifecycle helper.
//
// The virtual camera itself is a Windows 11 Media Foundation user-mode camera.
// No kernel driver is installed. Registering its COM media source and preparing
// the shared frame directory are machine-wide operations and therefore require
// a one-time UAC elevation. Creating/removing the virtual camera is per-user.

#include <windows.h>
#include <aclapi.h>
#include <mfapi.h>
#include <mfidl.h>
#include <mfvirtualcamera.h>
#include <sddl.h>
#include <shellapi.h>
#include <wrl/client.h>

#include <iostream>
#include <string>
#include <string_view>
#include <vector>

#pragma comment(lib, "mfplat.lib")
#pragma comment(lib, "mfuuid.lib")
#pragma comment(lib, "mfsensorgroup.lib")
#pragma comment(lib, "ole32.lib")
#pragma comment(lib, "advapi32.lib")
#pragma comment(lib, "shell32.lib")

using Microsoft::WRL::ComPtr;

namespace
{
// {09DD46EB-A361-4E8D-BC51-347A934A79C7}
constexpr GUID kCameraKindAttribute = {
    0x09dd46eb, 0xa361, 0x4e8d, {0xbc, 0x51, 0x34, 0x7a, 0x93, 0x4a, 0x79, 0xc7}};

constexpr wchar_t kMediaSourceClsidText[] = L"{D73E9C6C-648A-41F1-8EF8-D4B479212BE4}";
constexpr wchar_t kLegacyMediaSourceClsidText[] = L"{25F68372-1893-4772-9D42-C0AE438CD69B}";
constexpr wchar_t kMediaSourceRegistryKey[] =
    L"Software\\Classes\\CLSID\\{D73E9C6C-648A-41F1-8EF8-D4B479212BE4}\\InProcServer32";
constexpr wchar_t kFriendlyName[] = L"CameraTuner Virtual Camera";
constexpr wchar_t kStateKey[] = L"Software\\CameraTuner\\VirtualCamera";
constexpr wchar_t kFrameDirectory[] = L"C:\\ProgramData\\CameraTuner";
constexpr wchar_t kBuiltinUsersSid[] = L"S-1-5-32-545";
constexpr wchar_t kInstalledDirectoryName[] = L"CameraTuner";
constexpr wchar_t kInstalledMediaSourceName[] = L"camera-tuner-media-source-v10.dll";
constexpr wchar_t kInstalledVersionValue[] = L"CameraTunerSourceVersion";
constexpr DWORD kInstalledSourceVersion = 10;

class Runtime
{
public:
    Runtime()
    {
        m_com = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
        if (SUCCEEDED(m_com))
        {
            m_mediaFoundation = MFStartup(MF_VERSION, MFSTARTUP_FULL);
        }
    }

    ~Runtime()
    {
        if (SUCCEEDED(m_mediaFoundation))
        {
            MFShutdown();
        }
        if (SUCCEEDED(m_com))
        {
            CoUninitialize();
        }
    }

    HRESULT status() const
    {
        return FAILED(m_com) ? m_com : m_mediaFoundation;
    }

private:
    HRESULT m_com = E_FAIL;
    HRESULT m_mediaFoundation = E_FAIL;
};

bool HasInstallMarker()
{
    HKEY key = nullptr;
    const auto status = RegOpenKeyExW(HKEY_CURRENT_USER, kStateKey, 0, KEY_READ, &key);
    if (key != nullptr)
    {
        RegCloseKey(key);
    }
    return status == ERROR_SUCCESS;
}

bool FrameDirectoryExists()
{
    const DWORD attributes = GetFileAttributesW(kFrameDirectory);
    return attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
}

bool FileExists(const std::wstring& path)
{
    const DWORD attributes = GetFileAttributesW(path.c_str());
    return attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0;
}

HRESULT GetFullPath(const wchar_t* input, std::wstring& output)
{
    const DWORD required = GetFullPathNameW(input, 0, nullptr, nullptr);
    if (required == 0)
    {
        return HRESULT_FROM_WIN32(GetLastError());
    }
    std::vector<wchar_t> buffer(static_cast<size_t>(required) + 1);
    const DWORD written = GetFullPathNameW(input, static_cast<DWORD>(buffer.size()), buffer.data(), nullptr);
    if (written == 0 || written >= buffer.size())
    {
        return HRESULT_FROM_WIN32(written == 0 ? GetLastError() : ERROR_INSUFFICIENT_BUFFER);
    }
    output.assign(buffer.data(), written);
    return S_OK;
}

HRESULT GetInstalledMediaSourcePath(std::wstring& output)
{
    const DWORD required = GetEnvironmentVariableW(L"ProgramFiles", nullptr, 0);
    if (required == 0)
    {
        return HRESULT_FROM_WIN32(GetLastError());
    }
    std::vector<wchar_t> buffer(static_cast<size_t>(required) + 1);
    const DWORD written = GetEnvironmentVariableW(
        L"ProgramFiles",
        buffer.data(),
        static_cast<DWORD>(buffer.size()));
    if (written == 0 || written >= buffer.size())
    {
        return HRESULT_FROM_WIN32(written == 0 ? GetLastError() : ERROR_INSUFFICIENT_BUFFER);
    }
    output.assign(buffer.data(), written);
    output += L"\\";
    output += kInstalledDirectoryName;
    output += L"\\";
    output += kInstalledMediaSourceName;
    return S_OK;
}

bool ReadRegisteredMediaSourcePath(std::wstring& output)
{
    HKEY key = nullptr;
    const LSTATUS opened = RegOpenKeyExW(
        HKEY_LOCAL_MACHINE,
        kMediaSourceRegistryKey,
        0,
        KEY_QUERY_VALUE | KEY_WOW64_64KEY,
        &key);
    if (opened != ERROR_SUCCESS)
    {
        return false;
    }

    DWORD type = 0;
    DWORD byteCount = 0;
    LSTATUS queried = RegQueryValueExW(key, nullptr, nullptr, &type, nullptr, &byteCount);
    if (queried != ERROR_SUCCESS || (type != REG_SZ && type != REG_EXPAND_SZ) || byteCount < sizeof(wchar_t))
    {
        RegCloseKey(key);
        return false;
    }
    std::vector<wchar_t> value(byteCount / sizeof(wchar_t) + 1, L'\0');
    queried = RegQueryValueExW(
        key,
        nullptr,
        nullptr,
        &type,
        reinterpret_cast<BYTE*>(value.data()),
        &byteCount);
    RegCloseKey(key);
    if (queried != ERROR_SUCCESS)
    {
        return false;
    }
    value.back() = L'\0';
    output.assign(value.data());
    return !output.empty();
}

bool PathsEqual(const std::wstring& left, const std::wstring& right)
{
    return CompareStringOrdinal(
        left.c_str(),
        static_cast<int>(left.size()),
        right.c_str(),
        static_cast<int>(right.size()),
        TRUE) == CSTR_EQUAL;
}

bool IsMediaSourceRegistered()
{
    std::wstring registered;
    return ReadRegisteredMediaSourcePath(registered) && FileExists(registered);
}

bool IsMediaSourceInstalledSecurely()
{
    std::wstring expected;
    std::wstring registered;
    if (!(SUCCEEDED(GetInstalledMediaSourcePath(expected))
        && ReadRegisteredMediaSourcePath(registered)
        && PathsEqual(expected, registered)
        && FileExists(expected)))
    {
        return false;
    }
    HKEY key = nullptr;
    if (RegOpenKeyExW(
        HKEY_LOCAL_MACHINE,
        kMediaSourceRegistryKey,
        0,
        KEY_QUERY_VALUE | KEY_WOW64_64KEY,
        &key) != ERROR_SUCCESS)
    {
        return false;
    }
    DWORD type = 0;
    DWORD value = 0;
    DWORD valueSize = sizeof(value);
    const LSTATUS queried = RegQueryValueExW(
        key,
        kInstalledVersionValue,
        nullptr,
        &type,
        reinterpret_cast<BYTE*>(&value),
        &valueSize);
    RegCloseKey(key);
    return queried == ERROR_SUCCESS && type == REG_DWORD && value == kInstalledSourceVersion;
}

HRESULT ValidateMediaSourceActivation()
{
    CLSID clsid{};
    HRESULT result = CLSIDFromString(kMediaSourceClsidText, &clsid);
    if (FAILED(result))
    {
        return result;
    }
    ComPtr<IClassFactory> factory;
    return CoGetClassObject(
        clsid,
        CLSCTX_INPROC_SERVER,
        nullptr,
        IID_PPV_ARGS(&factory));
}

HRESULT SetInstallMarker(bool installed)
{
    if (!installed)
    {
        const auto result = RegDeleteTreeW(HKEY_CURRENT_USER, kStateKey);
        return result == ERROR_SUCCESS || result == ERROR_FILE_NOT_FOUND
            ? S_OK
            : HRESULT_FROM_WIN32(result);
    }

    HKEY key = nullptr;
    const auto result = RegCreateKeyExW(
        HKEY_CURRENT_USER,
        kStateKey,
        0,
        nullptr,
        REG_OPTION_NON_VOLATILE,
        KEY_WRITE,
        nullptr,
        &key,
        nullptr);
    if (result != ERROR_SUCCESS)
    {
        return HRESULT_FROM_WIN32(result);
    }
    constexpr DWORD version = 1;
    const auto writeResult = RegSetValueExW(
        key,
        L"SchemaVersion",
        0,
        REG_DWORD,
        reinterpret_cast<const BYTE*>(&version),
        sizeof(version));
    RegCloseKey(key);
    return HRESULT_FROM_WIN32(writeResult);
}

HRESULT RegisterMediaSource(const std::wstring& sourcePath)
{
    if (!FileExists(sourcePath))
    {
        return HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND);
    }

    HKEY key = nullptr;
    const LSTATUS created = RegCreateKeyExW(
        HKEY_LOCAL_MACHINE,
        kMediaSourceRegistryKey,
        0,
        nullptr,
        REG_OPTION_NON_VOLATILE,
        KEY_SET_VALUE | KEY_WOW64_64KEY,
        nullptr,
        &key,
        nullptr);
    if (created != ERROR_SUCCESS)
    {
        return HRESULT_FROM_WIN32(created);
    }

    const DWORD pathBytes = static_cast<DWORD>((sourcePath.size() + 1) * sizeof(wchar_t));
    LSTATUS written = RegSetValueExW(
        key,
        nullptr,
        0,
        REG_SZ,
        reinterpret_cast<const BYTE*>(sourcePath.c_str()),
        pathBytes);
    if (written == ERROR_SUCCESS)
    {
        constexpr wchar_t threadingModel[] = L"Both";
        written = RegSetValueExW(
            key,
            L"ThreadingModel",
            0,
            REG_SZ,
            reinterpret_cast<const BYTE*>(threadingModel),
            sizeof(threadingModel));
    }
    if (written == ERROR_SUCCESS)
    {
        written = RegSetValueExW(
            key,
            kInstalledVersionValue,
            0,
            REG_DWORD,
            reinterpret_cast<const BYTE*>(&kInstalledSourceVersion),
            sizeof(kInstalledSourceVersion));
    }
    RegCloseKey(key);
    return HRESULT_FROM_WIN32(written);
}

HRESULT InstallMediaSourceBinary(const std::wstring& sourcePath, std::wstring& installedPath)
{
    HRESULT result = GetInstalledMediaSourcePath(installedPath);
    if (FAILED(result))
    {
        return result;
    }
    const size_t separator = installedPath.find_last_of(L"\\/");
    if (separator == std::wstring::npos)
    {
        return E_UNEXPECTED;
    }
    const std::wstring directory = installedPath.substr(0, separator);
    if (PathsEqual(sourcePath, installedPath))
    {
        return FileExists(installedPath) ? S_OK : HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND);
    }
    if (!CreateDirectoryW(directory.c_str(), nullptr))
    {
        const DWORD error = GetLastError();
        if (error != ERROR_ALREADY_EXISTS)
        {
            return HRESULT_FROM_WIN32(error);
        }
    }
    if (!CopyFileW(sourcePath.c_str(), installedPath.c_str(), FALSE))
    {
        return HRESULT_FROM_WIN32(GetLastError());
    }
    return S_OK;
}

HRESULT EnsureFrameDirectory()
{
    if (!CreateDirectoryW(kFrameDirectory, nullptr))
    {
        const DWORD error = GetLastError();
        if (error != ERROR_ALREADY_EXISTS)
        {
            return HRESULT_FROM_WIN32(error);
        }
    }

    PSID usersSid = nullptr;
    if (!ConvertStringSidToSidW(kBuiltinUsersSid, &usersSid))
    {
        return HRESULT_FROM_WIN32(GetLastError());
    }

    PACL currentAcl = nullptr;
    PSECURITY_DESCRIPTOR descriptor = nullptr;
    DWORD result = GetNamedSecurityInfoW(
        const_cast<wchar_t*>(kFrameDirectory),
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        nullptr,
        nullptr,
        &currentAcl,
        nullptr,
        &descriptor);
    if (result != ERROR_SUCCESS)
    {
        LocalFree(usersSid);
        return HRESULT_FROM_WIN32(result);
    }

    EXPLICIT_ACCESSW access{};
    access.grfAccessPermissions = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;
    access.grfAccessMode = GRANT_ACCESS;
    access.grfInheritance = SUB_CONTAINERS_AND_OBJECTS_INHERIT;
    access.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    access.Trustee.TrusteeType = TRUSTEE_IS_WELL_KNOWN_GROUP;
    access.Trustee.ptstrName = static_cast<LPWSTR>(usersSid);

    PACL updatedAcl = nullptr;
    result = SetEntriesInAclW(1, &access, currentAcl, &updatedAcl);
    if (result == ERROR_SUCCESS)
    {
        result = SetNamedSecurityInfoW(
            const_cast<wchar_t*>(kFrameDirectory),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            nullptr,
            nullptr,
            updatedAcl,
            nullptr);
    }

    LocalFree(updatedAcl);
    LocalFree(descriptor);
    LocalFree(usersSid);
    return HRESULT_FROM_WIN32(result);
}

std::wstring QuoteArgument(const std::wstring& value)
{
    // A Windows path cannot contain a quote, so wrapping is sufficient here.
    return L"\"" + value + L"\"";
}

HRESULT ElevatePrerequisiteSetup(const std::wstring& sourcePath)
{
    wchar_t executable[MAX_PATH]{};
    const DWORD length = GetModuleFileNameW(nullptr, executable, ARRAYSIZE(executable));
    if (length == 0 || length >= ARRAYSIZE(executable))
    {
        return HRESULT_FROM_WIN32(length == 0 ? GetLastError() : ERROR_INSUFFICIENT_BUFFER);
    }
    if (sourcePath.find(L'\"') != std::wstring::npos)
    {
        return E_INVALIDARG;
    }

    const std::wstring parameters = L"install-elevated " + QuoteArgument(sourcePath);
    SHELLEXECUTEINFOW execution{};
    execution.cbSize = sizeof(execution);
    execution.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
    execution.lpVerb = L"runas";
    execution.lpFile = executable;
    execution.lpParameters = parameters.c_str();
    execution.nShow = SW_HIDE;
    if (!ShellExecuteExW(&execution))
    {
        return HRESULT_FROM_WIN32(GetLastError());
    }
    if (execution.hProcess == nullptr)
    {
        return E_FAIL;
    }
    const DWORD wait = WaitForSingleObject(execution.hProcess, INFINITE);
    DWORD exitCode = 1;
    const BOOL readExitCode = GetExitCodeProcess(execution.hProcess, &exitCode);
    CloseHandle(execution.hProcess);
    if (wait != WAIT_OBJECT_0)
    {
        return HRESULT_FROM_WIN32(GetLastError());
    }
    if (!readExitCode)
    {
        return HRESULT_FROM_WIN32(GetLastError());
    }
    return exitCode == 0 ? S_OK : static_cast<HRESULT>(exitCode);
}

HRESULT PreparePrerequisites(const std::wstring& sourcePath)
{
    if (IsMediaSourceInstalledSecurely() && FrameDirectoryExists())
    {
        return S_OK;
    }
    HRESULT result = ElevatePrerequisiteSetup(sourcePath);
    if (SUCCEEDED(result) && (!IsMediaSourceInstalledSecurely() || !FrameDirectoryExists()))
    {
        result = HRESULT_FROM_WIN32(ERROR_INSTALL_FAILURE);
    }
    return result;
}

HRESULT CreateCamera(ComPtr<IMFVirtualCamera>& camera)
{
    const auto result = MFCreateVirtualCamera(
        MFVirtualCameraType_SoftwareCameraSource,
        MFVirtualCameraLifetime_System,
        MFVirtualCameraAccess_CurrentUser,
        kFriendlyName,
        kMediaSourceClsidText,
        nullptr,
        0,
        &camera);
    if (FAILED(result))
    {
        return result;
    }
    return camera->SetUINT32(kCameraKindAttribute, 0);
}

void RemoveLegacyCameraIfPresent()
{
    ComPtr<IMFVirtualCamera> camera;
    HRESULT result = MFCreateVirtualCamera(
        MFVirtualCameraType_SoftwareCameraSource,
        MFVirtualCameraLifetime_System,
        MFVirtualCameraAccess_CurrentUser,
        kFriendlyName,
        kLegacyMediaSourceClsidText,
        nullptr,
        0,
        &camera);
    if (SUCCEEDED(result))
    {
        result = camera->Start(nullptr);
    }
    if (SUCCEEDED(result))
    {
        camera->Remove();
    }
    if (camera)
    {
        camera->Shutdown();
    }
}

HRESULT Install()
{
    BOOL supported = FALSE;
    HRESULT result = MFIsVirtualCameraTypeSupported(
        MFVirtualCameraType_SoftwareCameraSource,
        &supported);
    if (FAILED(result) || !supported)
    {
        return FAILED(result) ? result : HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
    }
    RemoveLegacyCameraIfPresent();
    ComPtr<IMFVirtualCamera> camera;
    result = CreateCamera(camera);
    if (SUCCEEDED(result))
    {
        result = camera->Start(nullptr);
    }
    if (camera)
    {
        camera->Shutdown();
    }
    return SUCCEEDED(result) ? SetInstallMarker(true) : result;
}

HRESULT Remove()
{
    ComPtr<IMFVirtualCamera> camera;
    HRESULT result = CreateCamera(camera);
    if (SUCCEEDED(result))
    {
        result = camera->Start(nullptr);
    }
    if (SUCCEEDED(result))
    {
        result = camera->Remove();
    }
    if (camera)
    {
        camera->Shutdown();
    }
    if (SUCCEEDED(result))
    {
        result = SetInstallMarker(false);
    }
    return result;
}

void PrintFailure(HRESULT result)
{
    wchar_t* message = nullptr;
    FormatMessageW(
        FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
        nullptr,
        static_cast<DWORD>(result),
        0,
        reinterpret_cast<wchar_t*>(&message),
        0,
        nullptr);
    std::wcerr << L"Error de cámara virtual 0x" << std::hex << static_cast<unsigned long>(result);
    if (message != nullptr)
    {
        std::wcerr << L": " << message;
        LocalFree(message);
    }
    std::wcerr << L'\n';
}
} // namespace

int wmain(int argc, wchar_t** argv)
{
    if (argc < 2)
    {
        std::wcerr << L"Uso: camera-tuner-virtual-camera <status|install|remove> [media-source.dll]\n";
        return 2;
    }

    const std::wstring_view action(argv[1]);
    if (action == L"status")
    {
        if (!IsMediaSourceInstalledSecurely())
        {
            std::wcout << (IsMediaSourceRegistered() ? L"source-needs-repair" : L"source-not-registered");
        }
        else if (!FrameDirectoryExists())
        {
            std::wcout << L"storage-not-ready";
        }
        else
        {
            Runtime runtime;
            const HRESULT activation = SUCCEEDED(runtime.status())
                ? ValidateMediaSourceActivation()
                : runtime.status();
            if (FAILED(activation))
            {
                std::wcout << L"source-invalid";
            }
            else
            {
                std::wcout << (HasInstallMarker() ? L"installed" : L"not-installed");
            }
        }
        return 0;
    }

    if (action == L"install-elevated")
    {
        if (argc != 3)
        {
            return 2;
        }
        std::wstring sourcePath;
        HRESULT result = GetFullPath(argv[2], sourcePath);
        if (SUCCEEDED(result))
        {
            std::wstring installedPath;
            result = InstallMediaSourceBinary(sourcePath, installedPath);
            if (SUCCEEDED(result))
            {
                result = RegisterMediaSource(installedPath);
            }
        }
        if (SUCCEEDED(result))
        {
            result = EnsureFrameDirectory();
        }
        if (FAILED(result))
        {
            PrintFailure(result);
            return static_cast<int>(result);
        }
        return 0;
    }

    HRESULT result = S_OK;
    if (action == L"install")
    {
        if (argc != 3)
        {
            return 2;
        }
        std::wstring sourcePath;
        result = GetFullPath(argv[2], sourcePath);
        if (SUCCEEDED(result) && !FileExists(sourcePath))
        {
            result = HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND);
        }
        if (SUCCEEDED(result))
        {
            result = PreparePrerequisites(sourcePath);
        }
    }
    else if (action != L"remove" || argc != 2)
    {
        std::wcerr << L"Acción desconocida.\n";
        return 2;
    }

    Runtime runtime;
    if (SUCCEEDED(result))
    {
        result = runtime.status();
    }
    if (SUCCEEDED(result) && action == L"install")
    {
        result = Install();
    }
    else if (SUCCEEDED(result) && action == L"remove")
    {
        result = Remove();
    }

    if (FAILED(result))
    {
        PrintFailure(result);
        return 1;
    }
    std::wcout << L"ok";
    return 0;
}
