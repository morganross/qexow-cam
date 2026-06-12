using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Linq;
using System.Windows.Forms;

namespace CamTray
{
    static class Program
    {
        [System.Runtime.InteropServices.DllImport("kernel32.dll")]
        private static extern bool AttachConsole(int dwProcessId);
        private const int ATTACH_PARENT_PROCESS = -1;

        [System.Runtime.InteropServices.DllImport("kernel32.dll")]
        private static extern IntPtr GetStdHandle(int nStdHandle);
        private const int STD_OUTPUT_HANDLE = -11;
        private const int STD_ERROR_HANDLE = -12;

        [System.Runtime.InteropServices.DllImport("kernel32.dll")]
        private static extern int GetFileType(IntPtr hFile);
        private const int FILE_TYPE_DISK = 0x0001;
        private const int FILE_TYPE_CHAR = 0x0002;
        private const int FILE_TYPE_PIPE = 0x0003;

        internal static string GetBinDir()
        {
            string appData = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
            return Path.Combine(appData, "QexowCam", "bin");
        }

        private static void ExtractAllResources()
        {
            try
            {
                string binDir = GetBinDir();
                ExtractResource("cam-core.exe", Path.Combine(binDir, "cam-core.exe"));
                ExtractResource("daemon-entry.js", Path.Combine(binDir, "daemon-entry.js"));
                ExtractResource("query_threads.py", Path.Combine(binDir, "query_threads.py"));
                ExtractResource("remote_query_threads.py", Path.Combine(binDir, "remote_query_threads.py"));
                ExtractResource("remote_query_threads.js", Path.Combine(binDir, "remote_query_threads.js"));
            }
            catch (Exception ex)
            {
                MessageBox.Show("Fatal: Failed to extract resources: " + ex.Message, "CAM Resource Error", MessageBoxButtons.OK, MessageBoxIcon.Error);
                Environment.Exit(1);
            }
        }

        private static void ExtractResource(string resourceName, string targetPath)
        {
            var assembly = System.Reflection.Assembly.GetExecutingAssembly();
            string actualResourceName = assembly.GetManifestResourceNames()
                .FirstOrDefault(name => name.EndsWith(resourceName, StringComparison.OrdinalIgnoreCase));

            if (string.IsNullOrEmpty(actualResourceName))
            {
                throw new Exception("Resource not found in manifest: " + resourceName);
            }

            FileInfo fileInfo = new FileInfo(targetPath);
            using (Stream stream = assembly.GetManifestResourceStream(actualResourceName))
            {
                if (stream == null)
                {
                    throw new Exception("Manifest stream is null for " + actualResourceName);
                }

                if (fileInfo.Exists && fileInfo.Length == stream.Length)
                {
                    return;
                }

                string dir = Path.GetDirectoryName(targetPath);
                if (!Directory.Exists(dir))
                {
                    Directory.CreateDirectory(dir);
                }

                using (FileStream fileStream = new FileStream(targetPath, FileMode.Create, FileAccess.Write))
                {
                    byte[] buffer = new byte[8192];
                    int bytesRead;
                    while ((bytesRead = stream.Read(buffer, 0, buffer.Length)) > 0)
                    {
                        fileStream.Write(buffer, 0, bytesRead);
                    }
                }
            }
        }

        [STAThread]
        static void Main(string[] args)
        {
            ExtractAllResources();

            if (args.Length > 0)
            {
                RunCli(args);
            }
            else
            {
                Application.EnableVisualStyles();
                Application.SetCompatibleTextRenderingDefault(false);
                Application.Run(new TrayApplicationContext());
            }
        }

        private static void RunCli(string[] args)
        {
            try
            {
                IntPtr stdOut = GetStdHandle(STD_OUTPUT_HANDLE);
                int outType = GetFileType(stdOut);

                if (outType != FILE_TYPE_PIPE && outType != FILE_TYPE_DISK)
                {
                    AttachConsole(ATTACH_PARENT_PROCESS);
                }

                try
                {
                    var stdOutHandle = GetStdHandle(STD_OUTPUT_HANDLE);
                    if (stdOutHandle != IntPtr.Zero && stdOutHandle != (IntPtr)(-1))
                    {
                        var safeFileHandle = new Microsoft.Win32.SafeHandles.SafeFileHandle(stdOutHandle, false);
                        var fileStream = new FileStream(safeFileHandle, FileAccess.Write);
                        var standardOutput = new StreamWriter(fileStream, System.Text.Encoding.Default) { AutoFlush = true };
                        Console.SetOut(standardOutput);
                    }
                }
                catch {}

                try
                {
                    var stdErrHandle = GetStdHandle(STD_ERROR_HANDLE);
                    if (stdErrHandle != IntPtr.Zero && stdErrHandle != (IntPtr)(-1))
                    {
                        var safeFileHandleErr = new Microsoft.Win32.SafeHandles.SafeFileHandle(stdErrHandle, false);
                        var fileStreamErr = new FileStream(safeFileHandleErr, FileAccess.Write);
                        var standardError = new StreamWriter(fileStreamErr, System.Text.Encoding.Default) { AutoFlush = true };
                        Console.SetError(standardError);
                    }
                }
                catch {}

                string binDir = GetBinDir();
                string coreExe = Path.Combine(binDir, "cam-core.exe");
                if (!File.Exists(coreExe))
                {
                    Console.Error.WriteLine("Error: cam-core.exe not found at " + coreExe);
                    Environment.Exit(1);
                }

                string arguments = string.Join(" ", args.Select(a => a.Contains(" ") ? "\"" + a + "\"" : a));
                ProcessStartInfo psi = new ProcessStartInfo(coreExe, arguments)
                {
                    CreateNoWindow = true,
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true
                };

                using (Process process = Process.Start(psi))
                {
                    process.OutputDataReceived += (sender, e) => { if (e.Data != null) Console.Out.WriteLine(e.Data); };
                    process.ErrorDataReceived += (sender, e) => { if (e.Data != null) Console.Error.WriteLine(e.Data); };
                    process.BeginOutputReadLine();
                    process.BeginErrorReadLine();
                    process.WaitForExit();
                    try { Console.Out.Flush(); } catch {}
                    try { Console.Error.Flush(); } catch {}
                    Environment.Exit(process.ExitCode);
                }
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine("Failed to run CAM CLI: " + ex.Message);
                Environment.Exit(1);
            }
        }
    }

    public class TrayApplicationContext : ApplicationContext
    {
        private NotifyIcon trayIcon;
        private Form statusForm;
        private TableLayoutPanel grid;

        public TrayApplicationContext()
        {
            // Create Context Menu
            ContextMenuStrip contextMenu = new ContextMenuStrip();
            contextMenu.Items.Add("Status", null, Status_Click);
            contextMenu.Items.Add("Start Daemon", null, Start_Click);
            contextMenu.Items.Add("Stop Daemon", null, Stop_Click);
            contextMenu.Items.Add(new ToolStripSeparator());
            contextMenu.Items.Add("Exit", null, Exit_Click);

            // Initialize Tray Icon
            trayIcon = new NotifyIcon()
            {
                Icon = SystemIcons.Shield,
                ContextMenuStrip = contextMenu,
                Visible = true,
                Text = "Qexow CAM"
            };

            trayIcon.DoubleClick += Status_Click;
        }

        private void Status_Click(object sender, EventArgs e)
        {
            ShowStatusDialog();
        }

        private struct StatusItem
        {
            public bool Ok;
            public string Label;
            public string Detail;
        }

        private void ShowStatusDialog()
        {
            if (statusForm != null && !statusForm.IsDisposed)
            {
                statusForm.Focus();
                return;
            }

            statusForm = new Form();
            statusForm.Text = "CAM System Status";
            statusForm.Size = new Size(760, 520);
            statusForm.MinimumSize = new Size(600, 400);
            statusForm.StartPosition = FormStartPosition.CenterScreen;
            statusForm.BackColor = Color.FromArgb(20, 20, 30);
            statusForm.ForeColor = Color.White;
            statusForm.Font = new Font("Segoe UI", 9.5f);

            // Header Panel
            Panel headerPanel = new Panel();
            headerPanel.Dock = DockStyle.Top;
            headerPanel.Height = 60;
            headerPanel.BackColor = Color.FromArgb(15, 15, 25);
            headerPanel.Padding = new Padding(15, 12, 15, 12);

            Label titleLabel = new Label();
            titleLabel.Text = "QEXOW CAM SYSTEM STATUS";
            titleLabel.Font = new Font("Segoe UI", 12f, FontStyle.Bold);
            titleLabel.ForeColor = Color.FromArgb(0, 162, 232);
            titleLabel.AutoSize = true;
            headerPanel.Controls.Add(titleLabel);
            statusForm.Controls.Add(headerPanel);

            // Main Panel with Scroll
            Panel mainPanel = new Panel();
            mainPanel.Dock = DockStyle.Fill;
            mainPanel.AutoScroll = true;
            mainPanel.Padding = new Padding(20);
            statusForm.Controls.Add(mainPanel);

            grid = new TableLayoutPanel();
            grid.ColumnCount = 4;
            grid.Dock = DockStyle.Top;
            grid.AutoSize = true;
            grid.AutoSizeMode = AutoSizeMode.GrowAndShrink;
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 35F));  // Light
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 35F));  // Label
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 50F));  // Detail
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 100F)); // Action button
            mainPanel.Controls.Add(grid);

            // Bottom Panel
            Panel bottomPanel = new Panel();
            bottomPanel.Dock = DockStyle.Bottom;
            bottomPanel.Height = 55;
            bottomPanel.BackColor = Color.FromArgb(15, 15, 25);
            bottomPanel.Padding = new Padding(15, 10, 15, 10);

            Button btnRefresh = new Button();
            btnRefresh.Text = "Refresh";
            btnRefresh.FlatStyle = FlatStyle.Flat;
            btnRefresh.FlatAppearance.BorderSize = 0;
            btnRefresh.BackColor = Color.FromArgb(0, 120, 215);
            btnRefresh.ForeColor = Color.White;
            btnRefresh.Font = new Font("Segoe UI", 9.5f, FontStyle.Bold);
            btnRefresh.Width = 90;
            btnRefresh.Height = 35;
            btnRefresh.Dock = DockStyle.Left;
            btnRefresh.Click += (s, ev) => RefreshStatusList();
            btnRefresh.MouseEnter += (s, ev) => btnRefresh.BackColor = Color.FromArgb(0, 140, 240);
            btnRefresh.MouseLeave += (s, ev) => btnRefresh.BackColor = Color.FromArgb(0, 120, 215);
            bottomPanel.Controls.Add(btnRefresh);

            Button btnClose = new Button();
            btnClose.Text = "Close";
            btnClose.FlatStyle = FlatStyle.Flat;
            btnClose.FlatAppearance.BorderSize = 0;
            btnClose.BackColor = Color.FromArgb(48, 48, 64);
            btnClose.ForeColor = Color.White;
            btnClose.Width = 90;
            btnClose.Height = 35;
            btnClose.Dock = DockStyle.Right;
            btnClose.Click += (s, ev) => statusForm.Close();
            btnClose.MouseEnter += (s, ev) => btnClose.BackColor = Color.FromArgb(80, 80, 100);
            btnClose.MouseLeave += (s, ev) => btnClose.BackColor = Color.FromArgb(48, 48, 64);
            bottomPanel.Controls.Add(btnClose);

            statusForm.Controls.Add(bottomPanel);

            // Initial Load
            RefreshStatusList();

            statusForm.ShowDialog();
        }

        private void RefreshStatusList()
        {
            grid.Controls.Clear();
            grid.RowStyles.Clear();
            grid.RowCount = 0;

            string output = RunCamCommand("doctor");
            string raw = string.IsNullOrWhiteSpace(output) ? "" : output;
            string[] outputLines = raw.Replace("\r\n", "\n").Replace("\r", "\n").Split('\n');
            
            var items = new List<StatusItem>();
            foreach (string line in outputLines)
            {
                if (line.StartsWith("OK ") || line.StartsWith("BAD"))
                {
                    bool ok = line.StartsWith("OK");
                    string content = line.Substring(ok ? 3 : 4).Trim();
                    int colonIdx = content.IndexOf(':');
                    string label = colonIdx >= 0 ? content.Substring(0, colonIdx).Trim() : content;
                    string detail = colonIdx >= 0 ? content.Substring(colonIdx + 1).Trim() : "";
                    items.Add(new StatusItem { Ok = ok, Label = label, Detail = detail });
                }
            }

            int rowIdx = 0;
            foreach (var item in items)
            {
                grid.RowCount++;
                grid.RowStyles.Add(new RowStyle(SizeType.Absolute, 35F));

                // 1. Status Light
                Label light = new Label();
                light.Text = "●";
                light.Font = new Font("Segoe UI", 12f);
                light.ForeColor = item.Ok ? Color.LimeGreen : Color.OrangeRed;
                light.TextAlign = ContentAlignment.MiddleCenter;
                light.Dock = DockStyle.Fill;
                grid.Controls.Add(light, 0, rowIdx);

                // 2. Label
                Label lbl = new Label();
                lbl.Text = item.Label;
                lbl.Font = new Font("Segoe UI", 9.5f, FontStyle.Bold);
                lbl.ForeColor = Color.White;
                lbl.TextAlign = ContentAlignment.MiddleLeft;
                lbl.Dock = DockStyle.Fill;
                grid.Controls.Add(lbl, 1, rowIdx);

                // 3. Detail
                Label det = new Label();
                det.Text = item.Detail;
                det.Font = new Font("Segoe UI", 9f);
                det.ForeColor = Color.LightGray;
                det.TextAlign = ContentAlignment.MiddleLeft;
                det.Dock = DockStyle.Fill;
                grid.Controls.Add(det, 2, rowIdx);

                // 4. Action Button
                if (ShouldShowButton(item.Label, item.Ok))
                {
                    Button btn = new Button();
                    btn.Text = GetButtonText(item.Label, item.Ok);
                    btn.FlatStyle = FlatStyle.Flat;
                    btn.FlatAppearance.BorderSize = 0;
                    btn.BackColor = Color.FromArgb(48, 48, 64);
                    btn.ForeColor = Color.White;
                    btn.Font = new Font("Segoe UI", 8.5f);
                    btn.Height = 25;
                    btn.Dock = DockStyle.Fill;
                    btn.Click += (s, ev) => HandleAction(item.Label, item.Ok);
                    btn.MouseEnter += (s, ev) => btn.BackColor = Color.FromArgb(80, 80, 100);
                    btn.MouseLeave += (s, ev) => btn.BackColor = Color.FromArgb(48, 48, 64);
                    grid.Controls.Add(btn, 3, rowIdx);
                }

                rowIdx++;
            }
        }

        private bool ShouldShowButton(string label, bool ok)
        {
            if (label.Contains("Codex Desktop App")) return true;
            if (label.Contains("Codex CLI")) return true;
            if (label.Contains("Codex auth")) return !ok;
            if (label.Contains("CAM daemon")) return true;
            return false;
        }

        private string GetButtonText(string label, bool ok)
        {
            if (label.Contains("Codex Desktop App")) return ok ? "Open" : "Download";
            if (label.Contains("Codex CLI")) return ok ? "Update" : "Install";
            if (label.Contains("Codex auth")) return "Login";
            if (label.Contains("CAM daemon")) return ok ? "Stop" : "Start";
            return "Action";
        }

        private void HandleAction(string label, bool ok)
        {
            if (label.Contains("Codex Desktop App"))
            {
                if (ok)
                {
                    try
                    {
                        Process.Start("codex://");
                    }
                    catch
                    {
                        string localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
                        string candidate = Path.Combine(localAppData, "OpenAI", "Codex", "Codex.exe");
                        if (File.Exists(candidate)) Process.Start(candidate);
                        else Process.Start("https://chatgpt.com/download");
                    }
                }
                else
                {
                    Process.Start("https://chatgpt.com/download");
                }
            }
            else if (label.Contains("Codex CLI"))
            {
                ProcessStartInfo psi = new ProcessStartInfo("cmd.exe", "/c npm install -g @openai/codex-cli && pause")
                {
                    UseShellExecute = true,
                    WindowStyle = ProcessWindowStyle.Normal
                };
                try { Process.Start(psi); } catch (Exception ex) { MessageBox.Show("Failed to launch installer: " + ex.Message); }
            }
            else if (label.Contains("Codex auth"))
            {
                ProcessStartInfo psi = new ProcessStartInfo("cmd.exe", "/c codex login && pause")
                {
                    UseShellExecute = true,
                    WindowStyle = ProcessWindowStyle.Normal
                };
                try { Process.Start(psi); } catch (Exception ex) { MessageBox.Show("Failed to launch login: " + ex.Message); }
            }
            else if (label.Contains("CAM daemon"))
            {
                if (ok)
                {
                    RunCamCommand("daemon stop");
                }
                else
                {
                    RunCamCommand("daemon start");
                }
                RefreshStatusList();
            }
        }


        private void Start_Click(object sender, EventArgs e)
        {
            string output = RunCamCommand("daemon start");
            MessageBox.Show(output, "Start CAM Daemon", MessageBoxButtons.OK, MessageBoxIcon.Information);
        }

        private void Stop_Click(object sender, EventArgs e)
        {
            string output = RunCamCommand("daemon stop");
            MessageBox.Show(output, "Stop CAM Daemon", MessageBoxButtons.OK, MessageBoxIcon.Information);
        }

        private void Exit_Click(object sender, EventArgs e)
        {
            trayIcon.Visible = false;
            Application.Exit();
        }

        private string RunCamCommand(string arguments)
        {
            try
            {
                string binDir = Program.GetBinDir();
                string camExe = Path.Combine(binDir, "cam-core.exe");

                if (!File.Exists(camExe))
                {
                    camExe = "cam-core.exe"; // Fallback to PATH
                }

                ProcessStartInfo processInfo = new ProcessStartInfo(camExe, arguments)
                {
                    CreateNoWindow = true,
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    WindowStyle = ProcessWindowStyle.Hidden
                };

                using (Process process = Process.Start(processInfo))
                {
                    process.WaitForExit();
                    string output = process.StandardOutput.ReadToEnd();
                    string error = process.StandardError.ReadToEnd();
                    
                    if (!string.IsNullOrWhiteSpace(error))
                    {
                        return output + "\n" + error;
                    }
                    return string.IsNullOrWhiteSpace(output) ? "Command executed successfully." : output;
                }
            }
            catch (Exception ex)
            {
                return string.Format("Failed to run cam: {0}", ex.Message);
            }
        }
    }
}
