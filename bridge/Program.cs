using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Diagnostics;
using System.Text.Json;

namespace ControlWebcamBridge;

internal static class Program
{
    private const int MaxRequestChars = 1024 * 1024;
    private static readonly JsonSerializerOptions Json = new() { PropertyNamingPolicy = JsonNamingPolicy.CamelCase };

    [STAThread]
    private static int Main(string[] args)
    {
        AppDomain.CurrentDomain.UnhandledException += (_, eventArgs) =>
            BridgeLog.Write("fatal", "unhandled_exception", "Excepción no controlada en el puente DirectShow.", new
            {
                exception = eventArgs.ExceptionObject.ToString(),
                eventArgs.IsTerminating
            });
        BridgeLog.Write("info", "session.started", "Puente DirectShow iniciado.", new
        {
            processId = Environment.ProcessId,
            mode = args.FirstOrDefault() ?? "none",
            runtime = Environment.Version.ToString()
        });
        if (args.Length == 1 && string.Equals(args[0], "serve", StringComparison.OrdinalIgnoreCase))
            return Serve();
        var timer = Stopwatch.StartNew();
        try
        {
            object response = Execute(args);
            Console.WriteLine(JsonSerializer.Serialize(response, Json));
            BridgeLog.Write(response is BridgeResult { Ok: false } ? "warn" : "info", "request.completed", "Solicitud DirectShow completada.", new
            {
                command = args.FirstOrDefault(),
                durationMs = timer.ElapsedMilliseconds,
                ok = response is not BridgeResult { Ok: false }
            });
            return response is BridgeResult { Ok: false } ? 1 : 0;
        }
        catch (Exception exception)
        {
            BridgeLog.Exception("request.failed", exception, args.FirstOrDefault());
            Console.WriteLine(JsonSerializer.Serialize(new BridgeResult(false, exception.Message), Json));
            return 1;
        }
    }

    private static IReadOnlyList<CameraInfo> ListCameras()
    {
        var entries = DirectShow.Enumerate();
        try
        {
            var nameIndexes = new Dictionary<string, int>(StringComparer.OrdinalIgnoreCase);
            CameraInfo[] cameras = entries.Select(camera =>
            {
                nameIndexes.TryGetValue(camera.Name, out int index);
                nameIndexes[camera.Name] = index + 1;
                return new CameraInfo(camera.Path, camera.Name, index);
            }).ToArray();
            BridgeLog.Write("info", "cameras.enumerated", "Cámaras DirectShow enumeradas.", new
            {
                count = cameras.Length,
                names = cameras.Select(camera => camera.Name).ToArray()
            });
            return cameras;
        }
        finally { DirectShow.ReleaseAll(entries); }
    }

    private static IReadOnlyList<ControlInfo> GetControls(string path)
    {
        CameraEntry? camera = DirectShow.Find(path);
        if (camera is null) throw new InvalidOperationException("La cámara seleccionada ya no está disponible.");
        try
        {
            object filter = camera.Bind();
            try
            {
                var controls = new List<ControlInfo>();
                IAMCameraControl? cameraControl = DirectShow.GetInterface<IAMCameraControl>(filter);
                IAMVideoProcAmp? videoControl = DirectShow.GetInterface<IAMVideoProcAmp>(filter);
                try
                {
                    if (cameraControl is not null)
                        AddCameraControls(controls, cameraControl);
                    if (videoControl is not null)
                        AddVideoControls(controls, videoControl);
                }
                finally
                {
                    DirectShow.Release(videoControl);
                    DirectShow.Release(cameraControl);
                }
                BridgeLog.Write("info", "controls.enumerated", "Controles DirectShow enumerados.", new
                {
                    count = controls.Count,
                    controls = controls.Select(control => control.Id).ToArray()
                });
                return controls;
            }
            finally { DirectShow.Release(filter); }
        }
        finally { DirectShow.Release(camera.Moniker); }
    }

    private static BridgeResult SetControl(string path, string kind, int property, int value, bool automatic)
    {
        CameraEntry? camera = DirectShow.Find(path);
        if (camera is null) return new BridgeResult(false, "La cámara seleccionada ya no está disponible.");
        try
        {
            object filter = camera.Bind();
            try
            {
                return kind.ToLowerInvariant() switch
                {
                    "camera" => SetCameraControl(filter, property, value, automatic),
                    "video" => SetVideoControl(filter, property, value, automatic),
                    _ => new BridgeResult(false, "El tipo de control no es válido."),
                };
            }
            finally { DirectShow.Release(filter); }
        }
        finally { DirectShow.Release(camera.Moniker); }
    }

    private static BridgeResult OpenPropertyPage(string path)
    {
        CameraEntry? camera = DirectShow.Find(path);
        if (camera is null) return new BridgeResult(false, "La cámara seleccionada ya no está disponible.");
        try
        {
            object filter = camera.Bind();
            try
            {
                ISpecifyPropertyPages? pages = DirectShow.GetInterface<ISpecifyPropertyPages>(filter);
                if (pages is null)
                    return new BridgeResult(false, "El controlador no expone una página de propiedades original.");
                try
                {
                    int getPagesResult = pages.GetPages(out CaUuid pageIds);
                    if (getPagesResult < 0) return FromHResult(getPagesResult);
                    if (pageIds.Count == 0 || pageIds.Elements == IntPtr.Zero)
                        return new BridgeResult(false, "El controlador devolvió una página de propiedades vacía.");
                    try
                    {
                        IntPtr owner = NativeMethods.GetForegroundWindow();
                        int result = NativeMethods.OleCreatePropertyFrame(
                            owner, 0, 0, $"{camera.Name} — propiedades del controlador",
                            1, ref filter, pageIds.Count, pageIds.Elements, 0, 0, IntPtr.Zero);
                        BridgeLog.Write(result < 0 ? "warn" : "info", "property_page.closed", "Página original del controlador cerrada.", new { result });
                        return FromHResult(result);
                    }
                    finally { Marshal.FreeCoTaskMem(pageIds.Elements); }
                }
                finally { DirectShow.Release(pages); }
            }
            finally { DirectShow.Release(filter); }
        }
        finally { DirectShow.Release(camera.Moniker); }
    }

    private static BridgeResult SetCameraControl(object filter, int property, int value, bool automatic)
    {
        if (!Enum.IsDefined(typeof(CameraControlProperty), property))
            return new BridgeResult(false, "La propiedad de cámara no es válida.");
        IAMCameraControl? control = DirectShow.GetInterface<IAMCameraControl>(filter);
        if (control is null) return new BridgeResult(false, "La cámara no expone controles de cámara.");
        try
        {
            CameraControlProperty selected = (CameraControlProperty)property;
            int rangeResult = control.GetRange(selected, out int min, out int max, out int step, out _, out ControlFlags caps);
            BridgeResult? validation = ValidateControl(rangeResult, min, max, step, caps, value, automatic);
            if (validation is not null) return validation;
            int result = control.Set(selected, value, automatic ? ControlFlags.Auto : ControlFlags.Manual);
            return FromHResult(result);
        }
        finally { DirectShow.Release(control); }
    }

    private static BridgeResult SetVideoControl(object filter, int property, int value, bool automatic)
    {
        if (!Enum.IsDefined(typeof(VideoProcAmpProperty), property))
            return new BridgeResult(false, "La propiedad de imagen no es válida.");
        IAMVideoProcAmp? control = DirectShow.GetInterface<IAMVideoProcAmp>(filter);
        if (control is null) return new BridgeResult(false, "La cámara no expone controles de imagen.");
        try
        {
            VideoProcAmpProperty selected = (VideoProcAmpProperty)property;
            int rangeResult = control.GetRange(selected, out int min, out int max, out int step, out _, out ControlFlags caps);
            BridgeResult? validation = ValidateControl(rangeResult, min, max, step, caps, value, automatic);
            if (validation is not null) return validation;
            int result = control.Set(selected, value, automatic ? ControlFlags.Auto : ControlFlags.Manual);
            return FromHResult(result);
        }
        finally { DirectShow.Release(control); }
    }

    private static BridgeResult? ValidateControl(int rangeResult, int min, int max, int step, ControlFlags caps, int value, bool automatic)
    {
        if (rangeResult < 0) return new BridgeResult(false, $"No se pudo consultar el control (0x{rangeResult:X8}).");
        ControlFlags required = automatic ? ControlFlags.Auto : ControlFlags.Manual;
        if ((caps & required) == 0) return new BridgeResult(false, automatic ? "El control no admite modo automático." : "El control no admite modo manual.");
        if (value < min || value > max) return new BridgeResult(false, $"El valor debe estar entre {min} y {max}.");
        if (step > 0 && (value - min) % step != 0) return new BridgeResult(false, $"El valor no respeta el incremento requerido de {step}.");
        return null;
    }

    private static BridgeResult FromHResult(int result) => result >= 0
        ? new BridgeResult(true, null)
        : new BridgeResult(false, $"El controlador rechazó el cambio (0x{result:X8}).");

    private static void AddCameraControls(List<ControlInfo> controls, IAMCameraControl control)
    {
        foreach (var item in new[]
        {
            (CameraControlProperty.Exposure, "exposure", "Exposición"),
            (CameraControlProperty.Zoom, "zoom", "Zoom"),
            (CameraControlProperty.Iris, "iris", "Iris"),
            (CameraControlProperty.Focus, "focus", "Enfoque"),
        })
        {
            if (control.GetRange(item.Item1, out int min, out int max, out int step, out int defaultValue, out ControlFlags caps) < 0) continue;
            if (control.Get(item.Item1, out int value, out ControlFlags flags) < 0) continue;
            bool supportsAuto = (caps & ControlFlags.Auto) != 0;
            bool supportsManual = (caps & ControlFlags.Manual) != 0;
            int normalizedValue = value < min || value > max ? defaultValue : value;
            controls.Add(new ControlInfo(item.Item2, item.Item3, "camera", (int)item.Item1, min, max, step, defaultValue, normalizedValue, (flags & ControlFlags.Auto) != 0, supportsAuto, supportsManual, supportsAuto && !supportsManual));
        }
    }

    private static void AddVideoControls(List<ControlInfo> controls, IAMVideoProcAmp control)
    {
        foreach (var item in new[]
        {
            (VideoProcAmpProperty.Brightness, "brightness", "Brillo"),
            (VideoProcAmpProperty.Contrast, "contrast", "Contraste"),
            (VideoProcAmpProperty.Hue, "hue", "Tono"),
            (VideoProcAmpProperty.Saturation, "saturation", "Saturación"),
            (VideoProcAmpProperty.Sharpness, "sharpness", "Nitidez"),
            (VideoProcAmpProperty.Gamma, "gamma", "Gamma"),
            (VideoProcAmpProperty.WhiteBalance, "whiteBalance", "Balance de blancos"),
            (VideoProcAmpProperty.BacklightCompensation, "backlight", "Luz de fondo"),
            (VideoProcAmpProperty.Gain, "gain", "Ganancia"),
        })
        {
            if (control.GetRange(item.Item1, out int min, out int max, out int step, out int defaultValue, out ControlFlags caps) < 0) continue;
            if (control.Get(item.Item1, out int value, out ControlFlags flags) < 0) continue;
            bool supportsAuto = (caps & ControlFlags.Auto) != 0;
            bool supportsManual = (caps & ControlFlags.Manual) != 0;
            int normalizedValue = value < min || value > max ? defaultValue : value;
            controls.Add(new ControlInfo(item.Item2, item.Item3, "video", (int)item.Item1, min, max, step, defaultValue, normalizedValue, (flags & ControlFlags.Auto) != 0, supportsAuto, supportsManual, supportsAuto && !supportsManual));
        }
    }

    private static int Serve()
    {
        BridgeLog.Write("info", "server.started", "Servidor persistente DirectShow listo.", null);
        string? line;
        while ((line = Console.ReadLine()) is not null)
        {
            object response;
            string? command = null;
            var timer = Stopwatch.StartNew();
            try
            {
                if (line.Length > MaxRequestChars)
                    throw new InvalidDataException("La solicitud supera el límite de 1 MiB.");
                BridgeRequest? request = JsonSerializer.Deserialize<BridgeRequest>(line, Json);
                command = request?.Command;
                response = request is null
                    ? new BridgeResult(false, "La solicitud está vacía.")
                    : Execute(new[] { request.Command }.Concat(request.Args ?? Array.Empty<string>()).ToArray());
                BridgeLog.Write(response is BridgeResult { Ok: false } ? "warn" : "debug", "request.completed", "Solicitud DirectShow completada.", new
                {
                    command,
                    durationMs = timer.ElapsedMilliseconds,
                    ok = response is not BridgeResult { Ok: false }
                });
            }
            catch (Exception exception)
            {
                BridgeLog.Exception("request.failed", exception, command);
                response = new BridgeResult(false, exception.Message);
            }
            Console.WriteLine(JsonSerializer.Serialize(response, Json));
            Console.Out.Flush();
        }
        BridgeLog.Write("info", "server.stopped", "La entrada del servidor DirectShow se cerró.", null);
        return 0;
    }

    private static object Execute(string[] args) => args.FirstOrDefault()?.ToLowerInvariant() switch
    {
        "list" => ListCameras(),
        "controls" when args.Length >= 2 => GetControls(args[1]),
        "set" when args.Length >= 6 => SetControl(args[1], args[2], int.Parse(args[3]), int.Parse(args[4]), bool.Parse(args[5])),
        "property-page" when args.Length >= 2 => OpenPropertyPage(args[1]),
        _ => new BridgeResult(false, "Comando inválido."),
    };
}

internal static class BridgeLog
{
    private static readonly object Sync = new();

    internal static void Write(string level, string eventName, string message, object? context)
    {
        try
        {
            var record = new
            {
                level,
                @event = eventName,
                message,
                bridgeTimestampMs = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(),
                threadId = Environment.CurrentManagedThreadId,
                context
            };
            lock (Sync)
            {
                Console.Error.WriteLine(JsonSerializer.Serialize(record));
                Console.Error.Flush();
            }
        }
        catch
        {
            // Diagnostics must never interrupt the control protocol.
        }
    }

    internal static void Exception(string eventName, Exception exception, string? command)
    {
        Write("error", eventName, exception.Message, new
        {
            command,
            exceptionType = exception.GetType().FullName,
            exception.StackTrace,
            inner = exception.InnerException?.Message
        });
    }
}

internal sealed record CameraInfo(string Id, string Name, int DeviceIndex);
internal sealed record ControlInfo(string Id, string Name, string Kind, int Property, int Minimum, int Maximum, int Step, int DefaultValue, int Value, bool Automatic, bool SupportsAuto, bool SupportsManual, bool DefaultAutomatic);
internal sealed record BridgeResult(bool Ok, string? Error);
internal sealed record BridgeRequest(string Command, string[]? Args);

[StructLayout(LayoutKind.Sequential)]
internal struct CaUuid
{
    internal uint Count;
    internal IntPtr Elements;
}

internal static class NativeMethods
{
    [DllImport("user32.dll")]
    internal static extern IntPtr GetForegroundWindow();

    [DllImport("oleaut32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    internal static extern int OleCreatePropertyFrame(
        IntPtr owner,
        uint x,
        uint y,
        string caption,
        uint objectCount,
        [MarshalAs(UnmanagedType.Interface)] ref object target,
        uint pageCount,
        IntPtr pageIds,
        uint locale,
        uint reserved,
        IntPtr reservedPointer);
}
internal sealed record CameraEntry(string Name, string Path, IMoniker Moniker)
{
    internal object Bind()
    {
        Guid iid = typeof(IBaseFilter).GUID;
        Moniker.BindToObject(null!, null!, ref iid, out object filter);
        return filter;
    }
}

internal static class DirectShow
{
    private static readonly Guid VideoInputCategory = new("860BB310-5D01-11D0-BD3B-00A0C911CE86");

    internal static List<CameraEntry> Enumerate()
    {
        var cameras = new List<CameraEntry>();
        var enumerator = (ICreateDevEnum)new CreateDevEnum();
        try
        {
            Guid category = VideoInputCategory;
            if (enumerator.CreateClassEnumerator(ref category, out IEnumMoniker? monikers, 0) != 0 || monikers is null) return cameras;
            try
            {
                var current = new IMoniker[1];
                while (monikers.Next(1, current, IntPtr.Zero) == 0)
                {
                    string name = FriendlyName(current[0]);
                    cameras.Add(new CameraEntry(name, DevicePath(current[0], name, cameras.Count), current[0]));
                    current = new IMoniker[1];
                }
            }
            finally { Release(monikers); }
        }
        finally { Release(enumerator); }
        return cameras;
    }

    internal static CameraEntry? Find(string path)
    {
        List<CameraEntry> cameras = Enumerate();
        CameraEntry? selected = cameras.FirstOrDefault(camera => string.Equals(camera.Path, path, StringComparison.OrdinalIgnoreCase));
        foreach (CameraEntry camera in cameras)
            if (!ReferenceEquals(camera, selected)) Release(camera.Moniker);
        return selected;
    }

    internal static void ReleaseAll(IEnumerable<CameraEntry> cameras)
    {
        foreach (CameraEntry camera in cameras) Release(camera.Moniker);
    }

    internal static T? GetInterface<T>(object source) where T : class
    {
        IntPtr unknown = IntPtr.Zero;
        IntPtr requested = IntPtr.Zero;
        try
        {
            unknown = Marshal.GetIUnknownForObject(source);
            Guid iid = typeof(T).GUID;
            int hr = Marshal.QueryInterface(unknown, in iid, out requested);
            return hr < 0 ? null : (T)Marshal.GetObjectForIUnknown(requested);
        }
        finally
        {
            if (requested != IntPtr.Zero) Marshal.Release(requested);
            if (unknown != IntPtr.Zero) Marshal.Release(unknown);
        }
    }

    internal static void Release(object? item)
    {
        if (item is not null && Marshal.IsComObject(item))
            try { Marshal.ReleaseComObject(item); } catch { }
    }

    private static string FriendlyName(IMoniker moniker) => ReadProperty(moniker, "FriendlyName", "Webcam sin nombre");
    private static string DevicePath(IMoniker moniker, string friendlyName, int deviceIndex)
    {
        string path = ReadProperty(moniker, "DevicePath", string.Empty);
        if (!string.IsNullOrWhiteSpace(path)) return path;
        try
        {
            moniker.GetDisplayName(null!, null!, out string displayName);
            return displayName;
        }
        catch
        {
            // DirectShow normally exposes DevicePath or the moniker display name.  A
            // deterministic fallback keeps the same camera addressable across the
            // separate bridge invocations used by the Tauri application.
            return $"fallback:{friendlyName}:{deviceIndex}";
        }
    }

    private static string ReadProperty(IMoniker moniker, string name, string fallback)
    {
        object? bagObject = null;
        try
        {
            Guid iid = typeof(IPropertyBag).GUID;
            moniker.BindToStorage(null!, null!, ref iid, out bagObject);
            object value = fallback;
            return ((IPropertyBag)bagObject).Read(name, ref value, IntPtr.Zero) >= 0 ? Convert.ToString(value) ?? fallback : fallback;
        }
        catch { return fallback; }
        finally { Release(bagObject); }
    }
}

[ComImport, Guid("62BE5D10-60EB-11D0-BD3B-00A0C911CE86")]
internal class CreateDevEnum { }
[ComImport, Guid("29840822-5B84-11D0-BD3B-00A0C911CE86"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface ICreateDevEnum { [PreserveSig] int CreateClassEnumerator(ref Guid category, [MarshalAs(UnmanagedType.Interface)] out IEnumMoniker? enumerator, int flags); }
[ComImport, Guid("56A86895-0AD4-11CE-B03A-0020AF0BA770"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IBaseFilter { }
[ComImport, Guid("B196B28B-BAB4-101A-B69C-00AA00341D07"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface ISpecifyPropertyPages { [PreserveSig] int GetPages(out CaUuid pages); }
[ComImport, Guid("55272A00-42CB-11CE-8135-00AA004BB851"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IPropertyBag { [PreserveSig] int Read([MarshalAs(UnmanagedType.LPWStr)] string propertyName, [MarshalAs(UnmanagedType.Struct)] ref object value, IntPtr errorLog); [PreserveSig] int Write([MarshalAs(UnmanagedType.LPWStr)] string propertyName, [MarshalAs(UnmanagedType.Struct)] ref object value); }
[ComImport, Guid("C6E13360-30AC-11D0-A18C-00A0C9118956"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IAMVideoProcAmp { [PreserveSig] int GetRange(VideoProcAmpProperty property, out int min, out int max, out int steppingDelta, out int defaultValue, out ControlFlags capabilities); [PreserveSig] int Set(VideoProcAmpProperty property, int value, ControlFlags flags); [PreserveSig] int Get(VideoProcAmpProperty property, out int value, out ControlFlags flags); }
[ComImport, Guid("C6E13370-30AC-11D0-A18C-00A0C9118956"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IAMCameraControl { [PreserveSig] int GetRange(CameraControlProperty property, out int min, out int max, out int steppingDelta, out int defaultValue, out ControlFlags capabilities); [PreserveSig] int Set(CameraControlProperty property, int value, ControlFlags flags); [PreserveSig] int Get(CameraControlProperty property, out int value, out ControlFlags flags); }
[Flags] internal enum ControlFlags { None = 0, Auto = 1, Manual = 2 }
internal enum VideoProcAmpProperty { Brightness, Contrast, Hue, Saturation, Sharpness, Gamma, ColorEnable, WhiteBalance, BacklightCompensation, Gain }
internal enum CameraControlProperty { Pan, Tilt, Roll, Zoom, Exposure, Iris, Focus, ScanMode, Privacy }
