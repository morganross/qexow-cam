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
        private TableLayoutPanel mappingsGrid;
        private TextBox txtLogReadout;

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
            statusForm.Size = new Size(1100, 750);
            statusForm.MinimumSize = new Size(800, 600);
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
            titleLabel.Text = "QEXOW CAM SYSTEM DASHBOARD";
            titleLabel.Font = new Font("Segoe UI", 12f, FontStyle.Bold);
            titleLabel.ForeColor = Color.FromArgb(0, 162, 232);
            titleLabel.AutoSize = true;
            headerPanel.Controls.Add(titleLabel);
            statusForm.Controls.Add(headerPanel);

            // Bottom Panel for Buttons
            Panel bottomPanel = new Panel();
            bottomPanel.Dock = DockStyle.Bottom;
            bottomPanel.Height = 55;
            bottomPanel.BackColor = Color.FromArgb(15, 15, 25);
            bottomPanel.Padding = new Padding(15, 10, 15, 10);

            Button btnRefresh = new Button();
            btnRefresh.Text = "Refresh All";
            btnRefresh.FlatStyle = FlatStyle.Flat;
            btnRefresh.FlatAppearance.BorderSize = 0;
            btnRefresh.BackColor = Color.FromArgb(0, 120, 215);
            btnRefresh.ForeColor = Color.White;
            btnRefresh.Font = new Font("Segoe UI", 9.5f, FontStyle.Bold);
            btnRefresh.Width = 110;
            btnRefresh.Height = 35;
            btnRefresh.Dock = DockStyle.Left;
            btnRefresh.Click += (s, ev) => {
                RefreshStatusList();
                RefreshAgentMappingsList();
                RefreshLogReadout();
            };
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

            // Outer SplitContainer splitting Top (checklist/mappings) and Bottom (logs)
            SplitContainer outerSplit = new SplitContainer();
            outerSplit.Dock = DockStyle.Fill;
            outerSplit.Orientation = Orientation.Horizontal;
            outerSplit.SplitterDistance = 430;
            outerSplit.Panel2MinSize = 150;
            outerSplit.Panel1MinSize = 200;
            outerSplit.BackColor = Color.FromArgb(20, 20, 30);
            statusForm.Controls.Add(outerSplit);

            // Inner SplitContainer splitting Left (checklist) and Right (mappings)
            SplitContainer innerSplit = new SplitContainer();
            innerSplit.Dock = DockStyle.Fill;
            innerSplit.Orientation = Orientation.Vertical;
            innerSplit.SplitterDistance = 530;
            innerSplit.Panel1MinSize = 250;
            innerSplit.Panel2MinSize = 250;
            innerSplit.BackColor = Color.FromArgb(20, 20, 30);
            outerSplit.Panel1.Controls.Add(innerSplit);

            // Left Panel: System Status Checklist
            Panel leftPanel = new Panel();
            leftPanel.Dock = DockStyle.Fill;
            leftPanel.Padding = new Padding(15);
            innerSplit.Panel1.Controls.Add(leftPanel);

            Label lblStatusTitle = new Label();
            lblStatusTitle.Text = "SYSTEM COMPONENT CHECKLIST";
            lblStatusTitle.Font = new Font("Segoe UI", 10f, FontStyle.Bold);
            lblStatusTitle.ForeColor = Color.FromArgb(180, 180, 200);
            lblStatusTitle.Dock = DockStyle.Top;
            lblStatusTitle.Height = 30;
            leftPanel.Controls.Add(lblStatusTitle);

            Panel leftScroll = new Panel();
            leftScroll.Dock = DockStyle.Fill;
            leftScroll.AutoScroll = true;
            leftPanel.Controls.Add(leftScroll);

            grid = new TableLayoutPanel();
            grid.ColumnCount = 4;
            grid.Dock = DockStyle.Top;
            grid.AutoSize = true;
            grid.AutoSizeMode = AutoSizeMode.GrowAndShrink;
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 30F));  // Light
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 45F));  // Label
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 55F));  // Detail
            grid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 85F));  // Action button
            leftScroll.Controls.Add(grid);

            // Right Panel: Agent Mappings
            Panel rightPanel = new Panel();
            rightPanel.Dock = DockStyle.Fill;
            rightPanel.Padding = new Padding(15);
            innerSplit.Panel2.Controls.Add(rightPanel);

            Label lblMappingsTitle = new Label();
            lblMappingsTitle.Text = "AGENT CHAT SESSION MAPPINGS";
            lblMappingsTitle.Font = new Font("Segoe UI", 10f, FontStyle.Bold);
            lblMappingsTitle.ForeColor = Color.FromArgb(180, 180, 200);
            lblMappingsTitle.Dock = DockStyle.Top;
            lblMappingsTitle.Height = 30;
            rightPanel.Controls.Add(lblMappingsTitle);

            Panel rightScroll = new Panel();
            rightScroll.Dock = DockStyle.Fill;
            rightScroll.AutoScroll = true;
            rightPanel.Controls.Add(rightScroll);

            mappingsGrid = new TableLayoutPanel();
            mappingsGrid.ColumnCount = 3;
            mappingsGrid.Dock = DockStyle.Top;
            mappingsGrid.AutoSize = true;
            mappingsGrid.AutoSizeMode = AutoSizeMode.GrowAndShrink;
            mappingsGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 35F)); // Agent Name
            mappingsGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 65F)); // Conversation ID
            mappingsGrid.ColumnStyles.Add(new ColumnStyle(SizeType.Absolute, 85F)); // Test button
            rightScroll.Controls.Add(mappingsGrid);

            // Bottom Panel: Log Readout
            Panel logsPanel = new Panel();
            logsPanel.Dock = DockStyle.Fill;
            logsPanel.Padding = new Padding(15, 5, 15, 10);
            outerSplit.Panel2.Controls.Add(logsPanel);

            Label lblLogTitle = new Label();
            lblLogTitle.Text = "LIVE CAM LOG READOUT (LAST 40 LINES)";
            lblLogTitle.Font = new Font("Segoe UI", 9f, FontStyle.Bold);
            lblLogTitle.ForeColor = Color.FromArgb(180, 180, 200);
            lblLogTitle.Dock = DockStyle.Top;
            lblLogTitle.Height = 25;
            logsPanel.Controls.Add(lblLogTitle);

            txtLogReadout = new TextBox();
            txtLogReadout.Multiline = true;
            txtLogReadout.ReadOnly = true;
            txtLogReadout.ScrollBars = ScrollBars.Vertical;
            txtLogReadout.BackColor = Color.FromArgb(10, 10, 15);
            txtLogReadout.ForeColor = Color.LightGreen;
            txtLogReadout.Font = new Font("Consolas", 8.5f);
            txtLogReadout.Dock = DockStyle.Fill;
            logsPanel.Controls.Add(txtLogReadout);

            // Initial Load
            RefreshStatusList();
            RefreshAgentMappingsList();
            RefreshLogReadout();

            statusForm.ShowDialog();
        }

        private void RefreshStatusList()
        {
            grid.Controls.Clear();
            grid.RowStyles.Clear();
            grid.RowCount = 0;

            var items = new List<StatusItem>();

            // 1. Antigravity Desktop App installed
            string localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            string agyAppDir = Path.Combine(localAppData, "Programs", "Antigravity");
            string agyAppExe = Path.Combine(agyAppDir, "Antigravity.exe");
            string agyLangSrv = Path.Combine(agyAppDir, "resources", "bin", "language_server.exe");

            bool hasAgyApp = Directory.Exists(agyAppDir);
            bool hasAgyExe = File.Exists(agyAppExe);
            bool hasAgyLangSrv = File.Exists(agyLangSrv);

            items.Add(new StatusItem { Ok = hasAgyApp, Label = "Antigravity Desktop App installed", Detail = hasAgyApp ? agyAppDir : "not found" });
            items.Add(new StatusItem { Ok = hasAgyExe, Label = "Antigravity Desktop App exe", Detail = hasAgyExe ? agyAppExe : "not found" });
            items.Add(new StatusItem { Ok = hasAgyLangSrv, Label = "Antigravity Language Server (agy)", Detail = hasAgyLangSrv ? agyLangSrv : "not found" });

            // 2. agy CLI in PATH
            string agyVer = RunAgyCommand("--version").Trim();
            bool hasAgyVer = !agyVer.StartsWith("failed") && !agyVer.Contains("timed out") && !string.IsNullOrWhiteSpace(agyVer);
            items.Add(new StatusItem { Ok = hasAgyVer, Label = "agy CLI in PATH", Detail = hasAgyVer ? agyVer : "NOT found — install Antigravity Desktop App" });

            // 3. Antigravity auth
            string agyStatus = RunAgyCommand("status").Trim();
            bool agyLoggedIn = hasAgyVer && !agyStatus.ToLower().Contains("unauthenticated") 
                                         && !agyStatus.ToLower().Contains("login required") 
                                         && !agyStatus.ToLower().Contains("not logged")
                                         && !agyStatus.StartsWith("failed");
            items.Add(new StatusItem { Ok = agyLoggedIn, Label = "Antigravity auth (agy status)", Detail = hasAgyVer ? (agyLoggedIn ? agyStatus.Split('\n')[0].Trim() : "NOT logged in — click Login") : "agy CLI not available" });

            // 4. Cascading CAM Doctor checks
            string camDoc = RunCamCommand("doctor");
            string raw = string.IsNullOrWhiteSpace(camDoc) ? "" : camDoc;
            string[] outputLines = raw.Replace("\r\n", "\n").Replace("\r", "\n").Split('\n');
            
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
            if (label.Contains("Antigravity Desktop App")) return true;
            if (label.Contains("Antigravity Language Server")) return !ok;
            if (label.Contains("agy CLI in PATH")) return !ok;
            if (label.Contains("Antigravity auth")) return !ok;
            if (label.Contains("CAM daemon")) return true;
            if (label.Contains("Codex Desktop App")) return true;
            if (label.Contains("Codex CLI")) return true;
            if (label.Contains("Codex auth")) return !ok;
            return false;
        }

        private string GetButtonText(string label, bool ok)
        {
            if (label.Contains("Antigravity Desktop App")) return ok ? "Open" : "Download";
            if (label.Contains("Antigravity Language Server")) return "Download";
            if (label.Contains("agy CLI in PATH")) return "Install";
            if (label.Contains("Antigravity auth")) return "Login";
            if (label.Contains("CAM daemon")) return ok ? "Stop" : "Start";
            if (label.Contains("Codex Desktop App")) return ok ? "Open" : "Download";
            if (label.Contains("Codex CLI")) return ok ? "Update" : "Install";
            if (label.Contains("Codex auth")) return "Login";
            return "Action";
        }

        private void HandleAction(string label, bool ok)
        {
            if (label.Contains("Antigravity Desktop App"))
            {
                if (ok)
                {
                    try
                    {
                        Process.Start("antigravity://");
                    }
                    catch
                    {
                        string localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
                        string agyAppExe = Path.Combine(localAppData, "Programs", "Antigravity", "Antigravity.exe");
                        if (File.Exists(agyAppExe)) Process.Start(agyAppExe);
                        else Process.Start("https://antigravity.google/download");
                    }
                }
                else
                {
                    Process.Start("https://antigravity.google/download");
                }
            }
            else if (label.Contains("Antigravity Language Server") || label.Contains("agy CLI in PATH"))
            {
                if (label.Contains("agy CLI in PATH"))
                {
                    ProcessStartInfo psi = new ProcessStartInfo("powershell.exe", "-NoExit -Command \"irm https://antigravity.google/cli/install.ps1 | iex\"")
                    {
                        UseShellExecute = true,
                        WindowStyle = ProcessWindowStyle.Normal
                    };
                    try { Process.Start(psi); } catch (Exception ex) { MessageBox.Show("Failed to launch installer: " + ex.Message); }
                }
                else
                {
                    Process.Start("https://antigravity.google/download");
                }
            }
            else if (label.Contains("Antigravity auth"))
            {
                ProcessStartInfo psi = new ProcessStartInfo("cmd.exe", "/c agy login && pause")
                {
                    UseShellExecute = true,
                    WindowStyle = ProcessWindowStyle.Normal
                };
                try { Process.Start(psi); } catch (Exception ex) { MessageBox.Show("Failed to launch login: " + ex.Message); }
            }
            else if (label.Contains("CAM daemon"))
            {
                if (ok) RunCamCommand("daemon stop");
                else RunCamCommand("daemon start");
                RefreshStatusList();
            }
            else if (label.Contains("Codex Desktop App"))
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
        }

        private void RefreshAgentMappingsList()
        {
            mappingsGrid.Controls.Clear();
            mappingsGrid.RowStyles.Clear();
            mappingsGrid.RowCount = 0;

            var mappings = GetAgentMappings();
            int rowIdx = 0;

            if (mappings.Count == 0)
            {
                mappingsGrid.RowCount++;
                mappingsGrid.RowStyles.Add(new RowStyle(SizeType.Absolute, 35F));

                Label lblNoMappings = new Label();
                lblNoMappings.Text = "No active agent mappings found.";
                lblNoMappings.Font = new Font("Segoe UI", 9.5f, FontStyle.Italic);
                lblNoMappings.ForeColor = Color.DarkGray;
                lblNoMappings.TextAlign = ContentAlignment.MiddleLeft;
                lblNoMappings.Dock = DockStyle.Fill;
                
                mappingsGrid.Controls.Add(lblNoMappings, 0, rowIdx);
                mappingsGrid.SetColumnSpan(lblNoMappings, 3);
                return;
            }

            foreach (var kvp in mappings)
            {
                string agentName = kvp.Key;
                string conversationId = kvp.Value;

                mappingsGrid.RowCount++;
                mappingsGrid.RowStyles.Add(new RowStyle(SizeType.Absolute, 35F));

                // 1. Agent Name
                Label lblName = new Label();
                lblName.Text = agentName;
                lblName.Font = new Font("Segoe UI", 9.5f, FontStyle.Bold);
                lblName.ForeColor = Color.FromArgb(0, 162, 232);
                lblName.TextAlign = ContentAlignment.MiddleLeft;
                lblName.Dock = DockStyle.Fill;
                mappingsGrid.Controls.Add(lblName, 0, rowIdx);

                // 2. Conversation ID
                Label lblId = new Label();
                lblId.Text = conversationId;
                lblId.Font = new Font("Consolas", 9f);
                lblId.ForeColor = Color.LightGray;
                lblId.TextAlign = ContentAlignment.MiddleLeft;
                lblId.Dock = DockStyle.Fill;
                
                ToolTip toolTip = new ToolTip();
                toolTip.SetToolTip(lblId, conversationId);
                
                mappingsGrid.Controls.Add(lblId, 1, rowIdx);

                // 3. Test Button
                Button btnTest = new Button();
                btnTest.Text = "Test";
                btnTest.FlatStyle = FlatStyle.Flat;
                btnTest.FlatAppearance.BorderSize = 0;
                btnTest.BackColor = Color.FromArgb(0, 122, 204);
                btnTest.ForeColor = Color.White;
                btnTest.Font = new Font("Segoe UI", 8.5f, FontStyle.Bold);
                btnTest.Height = 25;
                btnTest.Dock = DockStyle.Fill;
                btnTest.Click += (s, ev) => RunStatusTest(agentName, conversationId, btnTest);
                btnTest.MouseEnter += (s, ev) => { if (btnTest.Enabled) btnTest.BackColor = Color.FromArgb(28, 151, 234); };
                btnTest.MouseLeave += (s, ev) => { if (btnTest.Enabled) btnTest.BackColor = Color.FromArgb(0, 122, 204); };
                
                mappingsGrid.Controls.Add(btnTest, 2, rowIdx);

                rowIdx++;
            }
        }

        private void RefreshLogReadout()
        {
            try
            {
                string camDir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".qexow-cam");
                string logFile = Path.Combine(camDir, "daemon.jsonl");
                if (File.Exists(logFile))
                {
                    using (var fs = new FileStream(logFile, FileMode.Open, FileAccess.Read, FileShare.ReadWrite))
                    {
                        long size = fs.Length;
                        long start = Math.Max(0, size - 40000); // last ~40KB
                        fs.Seek(start, SeekOrigin.Begin);
                        using (var reader = new StreamReader(fs))
                        {
                            string raw = reader.ReadToEnd();
                            string[] lines = raw.Split(new[] { '\n', '\r' }, StringSplitOptions.RemoveEmptyEntries);
                            int skip = Math.Max(0, lines.Length - 40);
                            string display = string.Join(Environment.NewLine, lines.Skip(skip));
                            txtLogReadout.Text = display;
                            txtLogReadout.SelectionStart = txtLogReadout.Text.Length;
                            txtLogReadout.ScrollToCaret();
                        }
                    }
                }
                else
                {
                    txtLogReadout.Text = "No logs generated yet. Ensure the daemon is running.";
                }
            }
            catch (Exception ex)
            {
                txtLogReadout.Text = "Error reading logs: " + ex.Message;
            }
        }

        private Dictionary<string, string> GetAgentMappings()
        {
            var mappings = new Dictionary<string, string>();
            try
            {
                string agentsFile = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".qexow-cam", "agents.json");
                if (File.Exists(agentsFile))
                {
                    string content = File.ReadAllText(agentsFile);
                    int startIdx = content.IndexOf("\"agents\":");
                    if (startIdx >= 0)
                    {
                        int openBrace = content.IndexOf("{", startIdx);
                        int braceCount = 1;
                        int endIdx = -1;
                        for (int i = openBrace + 1; i < content.Length; i++)
                        {
                            if (content[i] == '{') braceCount++;
                            else if (content[i] == '}') braceCount--;
                            if (braceCount == 0)
                            {
                                endIdx = i;
                                break;
                            }
                        }
                        
                        if (openBrace >= 0 && endIdx > openBrace)
                        {
                            string section = content.Substring(openBrace, endIdx - openBrace + 1);
                            var jsonObject = parseSimpleJson(section);
                            foreach (var agentName in jsonObject.Keys)
                            {
                                var agentProps = parseSimpleJson(jsonObject[agentName]);
                                if (agentProps.ContainsKey("threadId") && !string.IsNullOrEmpty(agentProps["threadId"]))
                                {
                                    mappings[agentName] = agentProps["threadId"].Trim('"');
                                }
                            }
                        }
                    }
                }
            }
            catch {}
            return mappings;
        }
        
        // Very basic JSON parser just to extract agent threadIds
        private Dictionary<string, string> parseSimpleJson(string json)
        {
            var result = new Dictionary<string, string>();
            bool inString = false;
            int depth = 0;
            int lastStart = -1;
            string currentKey = null;
            
            for (int i = 0; i < json.Length; i++)
            {
                if (json[i] == '"' && (i == 0 || json[i-1] != '\\'))
                {
                    inString = !inString;
                }
                
                if (!inString)
                {
                    if (json[i] == '{') depth++;
                    else if (json[i] == '}') depth--;
                    
                    if (depth == 1 && json[i] == ':')
                    {
                        if (lastStart >= 0)
                        {
                            currentKey = json.Substring(lastStart, i - lastStart).Trim(' ', '\n', '\r', '"');
                            lastStart = i + 1;
                        }
                    }
                    else if (depth == 1 && json[i] == ',')
                    {
                        if (currentKey != null && lastStart >= 0)
                        {
                            result[currentKey] = json.Substring(lastStart, i - lastStart).Trim();
                            currentKey = null;
                        }
                        lastStart = i + 1;
                    }
                    else if (depth == 0 && json[i] == '}')
                    {
                        if (currentKey != null && lastStart >= 0)
                        {
                            result[currentKey] = json.Substring(lastStart, i - lastStart).Trim();
                        }
                    }
                }
                else if (lastStart == -1 && json[i] == '"')
                {
                    lastStart = i;
                }
            }
            return result;
        }

        private async void RunStatusTest(string agentName, string conversationId, Button btnTest)
        {
            btnTest.Enabled = false;
            btnTest.Text = "Testing...";
            btnTest.BackColor = Color.Orange;

            try
            {
                string output = RunCamCommand("send " + agentName + " \"what is your status\"");
                if (output.Contains("Error"))
                {
                    btnTest.Text = "Failed!";
                    btnTest.BackColor = Color.Red;
                }
                else
                {
                    btnTest.Text = "Success!";
                    btnTest.BackColor = Color.LimeGreen;
                }
            }
            catch
            {
                btnTest.Text = "Failed!";
                btnTest.BackColor = Color.Red;
            }

            await System.Threading.Tasks.Task.Delay(3000);
            
            if (btnTest != null && !btnTest.IsDisposed)
            {
                btnTest.Text = "Test";
                btnTest.BackColor = Color.FromArgb(0, 122, 204);
                btnTest.Enabled = true;
            }
        }

        private string RunAgyCommand(string arguments)
        {
            try
            {
                string localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
                string agyExe = Path.Combine(localAppData, "Programs", "Antigravity", "resources", "bin", "language_server.exe");
                
                if (!File.Exists(agyExe)) return "failed: agy not found";

                ProcessStartInfo processInfo = new ProcessStartInfo(agyExe, arguments)
                {
                    CreateNoWindow = true,
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    WindowStyle = ProcessWindowStyle.Hidden
                };

                using (Process process = Process.Start(processInfo))
                {
                    if (!process.WaitForExit(5000))
                    {
                        process.Kill();
                        return "failed: timed out";
                    }
                    string output = process.StandardOutput.ReadToEnd();
                    return string.IsNullOrWhiteSpace(output) ? process.StandardError.ReadToEnd() : output;
                }
            }
            catch (Exception ex)
            {
                return "failed: " + ex.Message;
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
