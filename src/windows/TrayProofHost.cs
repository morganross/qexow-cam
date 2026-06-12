using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Text;
using System.Windows.Forms;

namespace QexowCamTrayProof
{
    static class Program
    {
        [STAThread]
        static void Main(string[] args)
        {
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            Application.Run(new TrayProofContext());
        }
    }

    sealed class TrayProofContext : ApplicationContext
    {
        private readonly List<NotifyIcon> icons = new List<NotifyIcon>();
        private readonly string logFile;
        private Form statusForm;

        public TrayProofContext()
        {
            logFile = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                ".qexow-cam",
                "logs",
                "tray-proof.log"
            );
            Directory.CreateDirectory(Path.GetDirectoryName(logFile));
            Log("proof-host-start pid=" + Process.GetCurrentProcess().Id);

            CreateIcons();
            Log("proof-host-ready iconCount=" + icons.Count);
            ShowStatusWindow();
        }

        private void CreateIcons()
        {
            AddIcon(1, "Native WinForms NotifyIcon", SystemIcons.Application, Color.RoyalBlue);
            AddIcon(2, "PowerShell/.NET tray-host analogue", SystemIcons.Information, Color.SeaGreen);
            AddIcon(3, "Direct Win32 Shell_NotifyIcon analogue", SystemIcons.Shield, Color.DarkOrange);
            AddIcon(4, "Node/Electron/Tauri tray analogue", SystemIcons.WinLogo, Color.MediumPurple);
            AddIcon(5, "Python desktop tray analogue", SystemIcons.Question, Color.Crimson);
            AddIcon(6, "Go/Rust native tray analogue", SystemIcons.Asterisk, Color.DarkCyan);
            AddIcon(7, "Browser-hosted local app launcher analogue", SystemIcons.Warning, Color.Goldenrod);
            AddIcon(8, "Windows task/toast/status UI analogue", SystemIcons.Error, Color.DimGray);
        }

        private void AddIcon(int index, string method, Icon baseIcon, Color color)
        {
            string title = "Qexow CAM " + index + " - " + method;
            ContextMenuStrip menu = new ContextMenuStrip();
            menu.Items.Add("Status", null, delegate { ShowStatusWindow(); });
            menu.Items.Add("Write Log Marker", null, delegate { Log("manual-marker icon=" + index + " method=\"" + method + "\""); });
            menu.Items.Add(new ToolStripSeparator());
            menu.Items.Add("Exit All Proof Icons", null, delegate { ExitThread(); });

            NotifyIcon icon = new NotifyIcon();
            icon.Icon = BuildNumberIcon(index, color, baseIcon);
            icon.Text = title.Length > 63 ? title.Substring(0, 63) : title;
            icon.ContextMenuStrip = menu;
            icon.Visible = true;
            icon.DoubleClick += delegate { ShowStatusWindow(); };
            icons.Add(icon);
            Log("icon-created index=" + index + " method=\"" + method + "\" text=\"" + icon.Text + "\"");
        }

        private static Icon BuildNumberIcon(int number, Color color, Icon fallback)
        {
            try
            {
                using (Bitmap bmp = new Bitmap(32, 32))
                using (Graphics g = Graphics.FromImage(bmp))
                using (SolidBrush bg = new SolidBrush(color))
                using (SolidBrush fg = new SolidBrush(Color.White))
                using (Font font = new Font("Segoe UI", 16, FontStyle.Bold, GraphicsUnit.Pixel))
                {
                    g.Clear(Color.Transparent);
                    g.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
                    g.FillEllipse(bg, 1, 1, 30, 30);
                    string text = number.ToString();
                    SizeF size = g.MeasureString(text, font);
                    g.DrawString(text, font, fg, (32 - size.Width) / 2, (32 - size.Height) / 2 - 1);
                    IntPtr handle = bmp.GetHicon();
                    Icon icon = (Icon)Icon.FromHandle(handle).Clone();
                    DestroyIcon(handle);
                    return icon;
                }
            }
            catch
            {
                return fallback;
            }
        }

        [System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true)]
        private static extern bool DestroyIcon(IntPtr hIcon);

        private void ShowStatusWindow()
        {
            if (statusForm != null && !statusForm.IsDisposed)
            {
                statusForm.WindowState = FormWindowState.Normal;
                statusForm.Activate();
                Log("status-window-focused");
                return;
            }

            statusForm = new Form();
            statusForm.Text = "Qexow CAM Tray Proof - 8 Icons";
            statusForm.StartPosition = FormStartPosition.CenterScreen;
            statusForm.Size = new Size(720, 420);
            statusForm.MinimumSize = new Size(560, 320);

            TextBox text = new TextBox();
            text.Dock = DockStyle.Fill;
            text.Multiline = true;
            text.ReadOnly = true;
            text.ScrollBars = ScrollBars.Both;
            text.Font = new Font("Consolas", 10);
            text.Text = BuildStatusText();
            statusForm.Controls.Add(text);

            Button refresh = new Button();
            refresh.Text = "Refresh";
            refresh.Dock = DockStyle.Bottom;
            refresh.Height = 36;
            refresh.Click += delegate { text.Text = BuildStatusText(); Log("status-refresh"); };
            statusForm.Controls.Add(refresh);

            statusForm.FormClosed += delegate { statusForm = null; Log("status-window-closed"); };
            statusForm.Show();
            Log("status-window-created");
        }

        private string BuildStatusText()
        {
            StringBuilder sb = new StringBuilder();
            sb.AppendLine("Qexow CAM tray proof host");
            sb.AppendLine("Process: " + Process.GetCurrentProcess().Id);
            sb.AppendLine("Executable: " + Application.ExecutablePath);
            sb.AppendLine("Log: " + logFile);
            sb.AppendLine("Icon count: " + icons.Count);
            sb.AppendLine();
            for (int i = 0; i < icons.Count; i++)
            {
                sb.AppendLine((i + 1) + ". " + icons[i].Text);
            }
            return sb.ToString();
        }

        protected override void ExitThreadCore()
        {
            Log("proof-host-exit");
            foreach (NotifyIcon icon in icons)
            {
                icon.Visible = false;
                icon.Dispose();
            }
            base.ExitThreadCore();
        }

        private void Log(string message)
        {
            string line = DateTime.UtcNow.ToString("o") + " " + message;
            try
            {
                File.AppendAllText(logFile, line + Environment.NewLine);
            }
            catch
            {
            }
        }
    }
}
