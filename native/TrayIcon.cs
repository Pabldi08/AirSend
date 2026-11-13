using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

namespace AirSend;

internal sealed class TrayIcon : IDisposable
{
    private const uint Callback = 0x8001;
    private const uint LeftButtonUp = 0x0202;
    private const uint RightButtonUp = 0x0205;
    private const uint ContextMenu = 0x007B;
    private readonly IntPtr window;
    private readonly IntPtr icon;
    private readonly uint taskbarCreated = RegisterWindowMessage("TaskbarCreated");
    private readonly Subclass procedure;
    private readonly Action toggle;
    private readonly Action quit;
    private NotifyIconData data;

    internal string ShowLabel { get; set; } = "";
    internal string QuitLabel { get; set; } = "";
    internal bool IsAvailable { get; private set; }

    internal TrayIcon(IntPtr window, string iconPath, Action toggle, Action quit)
    {
        this.window = window;
        this.toggle = toggle;
        this.quit = quit;
        icon = LoadImage(IntPtr.Zero, iconPath, 1, 0, 0, 0x50);
        if (icon == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        data = new NotifyIconData
        {
            Size = (uint)Marshal.SizeOf<NotifyIconData>(),
            Window = window,
            Id = 1,
            Flags = 7,
            CallbackMessage = Callback,
            Icon = icon,
            Tip = "AirSend",
            Info = "",
            InfoTitle = ""
        };
        procedure = HandleMessage;
        if (!SetWindowSubclass(window, procedure, 1, 0))
        {
            DestroyIcon(icon);
            throw new InvalidOperationException("Unable to attach the AirSend tray icon.");
        }
        IsAvailable = Shell_NotifyIcon(0, ref data);
    }

    private IntPtr HandleMessage(IntPtr hwnd, uint message, UIntPtr wParam, IntPtr lParam,
        UIntPtr id, UIntPtr reference)
    {
        if (message == taskbarCreated)
        {
            IsAvailable = Shell_NotifyIcon(0, ref data);
        }
        else if (message == Callback)
        {
            switch ((uint)lParam.ToInt64())
            {
                case LeftButtonUp: toggle(); break;
                case RightButtonUp:
                case ContextMenu: OpenMenu(); break;
            }
            return IntPtr.Zero;
        }
        return DefSubclassProc(hwnd, message, wParam, lParam);
    }

    private void OpenMenu()
    {
        var menu = CreatePopupMenu();
        if (menu == IntPtr.Zero) return;
        uint selected;
        try
        {
            AppendMenu(menu, 0, 1, ShowLabel);
            AppendMenu(menu, 0x800, 0, null);
            AppendMenu(menu, 0, 2, QuitLabel);
            GetCursorPos(out var point);
            SetForegroundWindow(window);
            selected = TrackPopupMenu(menu, 0x102, point.X, point.Y, 0, window, IntPtr.Zero);
            PostMessage(window, 0, UIntPtr.Zero, IntPtr.Zero);
        }
        finally { DestroyMenu(menu); }
        if (selected == 1) toggle();
        if (selected == 2) quit();
    }

    public void Dispose()
    {
        Shell_NotifyIcon(2, ref data);
        RemoveWindowSubclass(window, procedure, 1);
        DestroyIcon(icon);
        IsAvailable = false;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NotifyIconData
    {
        internal uint Size;
        internal IntPtr Window;
        internal uint Id, Flags, CallbackMessage;
        internal IntPtr Icon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] internal string Tip;
        internal uint State, StateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] internal string Info;
        internal uint TimeoutOrVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] internal string InfoTitle;
        internal uint InfoFlags;
        internal Guid Guid;
        internal IntPtr BalloonIcon;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Point { internal int X, Y; }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate IntPtr Subclass(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam, UIntPtr id, UIntPtr reference);

    [DllImport("comctl32.dll")] private static extern bool SetWindowSubclass(IntPtr window, Subclass callback, nuint id, nuint data);
    [DllImport("comctl32.dll")] private static extern bool RemoveWindowSubclass(IntPtr window, Subclass callback, nuint id);
    [DllImport("comctl32.dll")] private static extern IntPtr DefSubclassProc(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)] private static extern bool Shell_NotifyIcon(uint action, ref NotifyIconData data);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] private static extern IntPtr LoadImage(IntPtr instance, string name, uint type, int width, int height, uint flags);
    [DllImport("user32.dll")] private static extern bool DestroyIcon(IntPtr icon);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern uint RegisterWindowMessage(string name);
    [DllImport("user32.dll")] private static extern IntPtr CreatePopupMenu();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern bool AppendMenu(IntPtr menu, uint flags, nuint id, string? label);
    [DllImport("user32.dll")] private static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll")] private static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] private static extern uint TrackPopupMenu(IntPtr menu, uint flags, int x, int y, int reserved, IntPtr window, IntPtr rect);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern bool PostMessage(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] private static extern bool DestroyMenu(IntPtr menu);
}
