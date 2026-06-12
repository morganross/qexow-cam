using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Net;
using System.Text;
using System.Threading;
using System.Web.Script.Serialization;
using System.Windows.Forms;

namespace QexowCamGui
{
    static class Program
    {
        [STAThread]
        static void Main(string[] args)
        {
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            Application.Run(new CamAppContext());
        }
    }

    sealed class CamAppContext : ApplicationContext
    {
        private readonly NotifyIcon trayIcon;
        private readonly MainForm form;
        private readonly string logFile;

        public CamAppContext()
        {
            string root = CamPaths.Root;
            Directory.CreateDirectory(Path.Combine(root, "logs"));
            logFile = Path.Combine(root, "logs", "windows-gui.log");
            Log("gui-start pid=" + Process.GetCurrentProcess().Id + " exe=\"" + Application.ExecutablePath + "\"");
            StopOtherGuiInstances();

            trayIcon = new NotifyIcon();
            trayIcon.Icon = SystemIcons.Application;
            trayIcon.Text = "Qexow CAM";
            trayIcon.Visible = true;
            trayIcon.DoubleClick += delegate { ShowMainWindow("tray-double-click"); };

            ContextMenuStrip menu = new ContextMenuStrip();
            menu.Items.Add("Open Status Window", null, delegate { ShowMainWindow("tray-menu-open"); });
            menu.Items.Add("Refresh", null, delegate { ShowMainWindow("tray-menu-refresh"); form.RefreshAll(); });
            menu.Items.Add(new ToolStripSeparator());
            menu.Items.Add("Exit", null, delegate { ExitThread(); });
            trayIcon.ContextMenuStrip = menu;

            form = new MainForm(Log);
            form.FormClosing += delegate(object sender, FormClosingEventArgs e)
            {
                if (e.CloseReason == CloseReason.UserClosing)
                {
                    e.Cancel = true;
                    form.Hide();
                    Log("window-hidden");
                }
            };

            Log("tray-created");
            ShowMainWindow("startup");
        }

        private void ShowMainWindow(string reason)
        {
            Log("window-show reason=" + reason);
            if (!form.Visible) form.Show();
            if (form.WindowState == FormWindowState.Minimized) form.WindowState = FormWindowState.Normal;
            form.Activate();
            form.RefreshAll();
        }

        protected override void ExitThreadCore()
        {
            Log("gui-exit");
            trayIcon.Visible = false;
            trayIcon.Dispose();
            base.ExitThreadCore();
        }

        private void Log(string message)
        {
            try
            {
                File.AppendAllText(logFile, DateTime.UtcNow.ToString("o") + " " + message + Environment.NewLine);
            }
            catch
            {
            }
        }

        private void StopOtherGuiInstances()
        {
            int currentPid = Process.GetCurrentProcess().Id;
            foreach (Process process in Process.GetProcessesByName("qexow-cam-gui"))
            {
                try
                {
                    if (process.Id == currentPid) continue;
                    Log("gui-duplicate-stop pid=" + process.Id);
                    process.Kill();
                    process.WaitForExit(3000);
                }
                catch (Exception ex)
                {
                    Log("gui-duplicate-stop-failed pid=" + process.Id + " error=" + ex.Message);
                }
            }
        }
    }

    sealed class MainForm : Form
    {
        private readonly Action<string> log;
        private readonly Label daemonLabel;
        private readonly Panel daemonLight;
        private readonly DataGridView agentsGrid;
        private readonly TextBox outputBox;
        private readonly Button refreshButton;
        private readonly Button testButton;
        private readonly CheckBox showArchivedCheckBox;
        private readonly JavaScriptSerializer json = new JavaScriptSerializer();
        private Dictionary<string, Dictionary<string, object>> activeThreadMetadata = new Dictionary<string, Dictionary<string, object>>(StringComparer.OrdinalIgnoreCase);
        private bool daemonStartAttempted = false;

        public MainForm(Action<string> logger)
        {
            log = logger;
            Text = "Qexow CAM Status";
            StartPosition = FormStartPosition.CenterScreen;
            Size = new Size(1100, 720);
            MinimumSize = new Size(840, 560);
            Font = new Font("Segoe UI", 9.0f);

            TableLayoutPanel root = new TableLayoutPanel();
            root.Dock = DockStyle.Fill;
            root.RowCount = 4;
            root.ColumnCount = 1;
            root.RowStyles.Add(new RowStyle(SizeType.Absolute, 72));
            root.RowStyles.Add(new RowStyle(SizeType.Percent, 60));
            root.RowStyles.Add(new RowStyle(SizeType.Absolute, 48));
            root.RowStyles.Add(new RowStyle(SizeType.Percent, 40));
            Controls.Add(root);

            Panel header = new Panel();
            header.Dock = DockStyle.Fill;
            header.Padding = new Padding(16, 12, 16, 8);
            header.BackColor = Color.FromArgb(245, 247, 250);
            root.Controls.Add(header, 0, 0);

            Label title = new Label();
            title.Text = "Qexow CAM";
            title.Font = new Font("Segoe UI", 15.0f, FontStyle.Bold);
            title.AutoSize = true;
            title.Location = new Point(16, 12);
            header.Controls.Add(title);

            daemonLight = new Panel();
            daemonLight.Size = new Size(18, 18);
            daemonLight.Location = new Point(18, 44);
            daemonLight.BackColor = Color.Gray;
            header.Controls.Add(daemonLight);

            daemonLabel = new Label();
            daemonLabel.Text = "Checking daemon...";
            daemonLabel.AutoSize = true;
            daemonLabel.Location = new Point(44, 43);
            header.Controls.Add(daemonLabel);

            agentsGrid = new DataGridView();
            agentsGrid.Dock = DockStyle.Fill;
            agentsGrid.ReadOnly = true;
            agentsGrid.AllowUserToAddRows = false;
            agentsGrid.AllowUserToDeleteRows = false;
            agentsGrid.SelectionMode = DataGridViewSelectionMode.FullRowSelect;
            agentsGrid.MultiSelect = false;
            agentsGrid.AutoSizeColumnsMode = DataGridViewAutoSizeColumnsMode.Fill;
            agentsGrid.RowHeadersVisible = false;
            root.Controls.Add(agentsGrid, 0, 1);

            Panel buttons = new Panel();
            buttons.Dock = DockStyle.Fill;
            buttons.Padding = new Padding(10, 8, 10, 8);
            root.Controls.Add(buttons, 0, 2);

            refreshButton = new Button();
            refreshButton.Text = "Refresh";
            refreshButton.Width = 110;
            refreshButton.Dock = DockStyle.Left;
            refreshButton.Click += delegate { RefreshAll(); };
            buttons.Controls.Add(refreshButton);

            showArchivedCheckBox = new CheckBox();
            showArchivedCheckBox.Text = "Show archived/all";
            showArchivedCheckBox.Width = 150;
            showArchivedCheckBox.Dock = DockStyle.Left;
            showArchivedCheckBox.CheckedChanged += delegate { RefreshAll(); };
            buttons.Controls.Add(showArchivedCheckBox);

            testButton = new Button();
            testButton.Text = "Test Selected Agent";
            testButton.Width = 170;
            testButton.Dock = DockStyle.Left;
            testButton.Click += delegate { TestSelectedAgent(); };
            buttons.Controls.Add(testButton);

            outputBox = new TextBox();
            outputBox.Dock = DockStyle.Fill;
            outputBox.Multiline = true;
            outputBox.ReadOnly = true;
            outputBox.ScrollBars = ScrollBars.Both;
            outputBox.Font = new Font("Consolas", 9.0f);
            root.Controls.Add(outputBox, 0, 3);

            Load += delegate { RefreshAll(); };
        }

        public void RefreshAll()
        {
            log("refresh-start");
            outputBox.Text = AppendLine(outputBox.Text, "Refreshing CAM status...");
            ThreadPool.QueueUserWorkItem(delegate
            {
                try
                {
                    Dictionary<string, object> health = ApiGet("/health");
                    object nodeName = health.ContainsKey("nodeName") ? health["nodeName"] : "";
                    object startedAt = health.ContainsKey("startedAt") ? health["startedAt"] : "";
                    InvokeUi(delegate
                    {
                        daemonLight.BackColor = Color.LimeGreen;
                        daemonLabel.Text = "Daemon online - node=" + nodeName + " started=" + startedAt;
                    });
                    log("health-ok");
                }
                catch (Exception ex)
                {
                    if (!daemonStartAttempted && TryStartDaemon())
                    {
                        Thread.Sleep(2000);
                        try
                        {
                            Dictionary<string, object> retryHealth = ApiGet("/health");
                            object retryNodeName = retryHealth.ContainsKey("nodeName") ? retryHealth["nodeName"] : "";
                            object retryStartedAt = retryHealth.ContainsKey("startedAt") ? retryHealth["startedAt"] : "";
                            InvokeUi(delegate
                            {
                                daemonLight.BackColor = Color.LimeGreen;
                                daemonLabel.Text = "Daemon online - node=" + retryNodeName + " started=" + retryStartedAt;
                            });
                            log("health-ok-after-daemon-start");
                            goto LoadAgentList;
                        }
                        catch (Exception retryEx)
                        {
                            ex = retryEx;
                        }
                    }
                    InvokeUi(delegate
                    {
                        daemonLight.BackColor = Color.OrangeRed;
                        daemonLabel.Text = "Daemon offline/error - " + ex.Message;
                    });
                    log("health-error " + ex.Message);
                }

            LoadAgentList:
                try
                {
                    List<Dictionary<string, object>> agents = LoadAgents();
                    InvokeUi(delegate { RenderAgents(agents); });
                    log("agents-loaded count=" + agents.Count);
                }
                catch (Exception ex)
                {
                    InvokeUi(delegate { outputBox.Text = AppendLine(outputBox.Text, "Agent load error: " + ex.Message); });
                    log("agents-error " + ex.Message);
                }
            });
        }

        private void RenderAgents(List<Dictionary<string, object>> agents)
        {
            agentsGrid.Columns.Clear();
            agentsGrid.Rows.Clear();
            foreach (string column in new[] { "light", "chatTitle", "name", "status", "node", "threadId", "activeTurnId", "cwd", "model" })
            {
                agentsGrid.Columns.Add(column, column);
            }
            agentsGrid.Columns["light"].HeaderText = "";
            agentsGrid.Columns["chatTitle"].HeaderText = "chat";
            agentsGrid.Columns["name"].HeaderText = "agent mapping";
            agentsGrid.Columns["light"].Width = 34;
            agentsGrid.Columns["light"].FillWeight = 8;
            agentsGrid.Columns["chatTitle"].FillWeight = 28;
            agentsGrid.Columns["name"].FillWeight = 22;
            foreach (Dictionary<string, object> agent in agents)
            {
                int rowIndex = agentsGrid.Rows.Add(
                    "●",
                    DisplayTitle(agent),
                    Value(agent, "name"),
                    Value(agent, "status"),
                    Value(agent, "node"),
                    Value(agent, "threadId"),
                    Value(agent, "activeTurnId"),
                    Value(agent, "cwd"),
                    Value(agent, "model")
                );
                DataGridViewRow row = agentsGrid.Rows[rowIndex];
                string status = Value(agent, "status").ToLowerInvariant();
                Color color = status == "running" || status == "busy" || status == "active" ? Color.DodgerBlue :
                    status == "error" || status == "failed" ? Color.OrangeRed : Color.LimeGreen;
                row.Cells["light"].Style.ForeColor = color;
                row.Cells["light"].Style.Alignment = DataGridViewContentAlignment.MiddleCenter;
            }
            outputBox.Text = AppendLine(outputBox.Text, "Loaded " + agents.Count + " agent/session mappings.");
        }

        private void TestSelectedAgent()
        {
            if (agentsGrid.SelectedRows.Count == 0)
            {
                MessageBox.Show(this, "Select an agent first.", "Qexow CAM", MessageBoxButtons.OK, MessageBoxIcon.Information);
                return;
            }
            string agentName = Convert.ToString(agentsGrid.SelectedRows[0].Cells["name"].Value);
            if (string.IsNullOrWhiteSpace(agentName)) return;

            testButton.Enabled = false;
            outputBox.Text = AppendLine(outputBox.Text, "Sending test message to " + agentName + "...");
            log("test-start agent=" + agentName);

            ThreadPool.QueueUserWorkItem(delegate
            {
                try
                {
                    Dictionary<string, object> payload = new Dictionary<string, object>();
                    payload["targetAgent"] = agentName;
                    payload["sourceAgent"] = "windows-gui";
                    payload["sourceNode"] = Environment.MachineName;
                    payload["message"] = "CAM GUI test: reply with your agent name, node name, and current status.";

                    Dictionary<string, object> sendResult = ApiPost("/send", payload);
                    string pretty = json.Serialize(sendResult);
                    InvokeUi(delegate
                    {
                        outputBox.Text = AppendLine(outputBox.Text, "Message sent. Awaiting/recording response path...");
                        outputBox.Text = AppendLine(outputBox.Text, pretty);
                        MessageBox.Show(this, pretty, "Qexow CAM test result", MessageBoxButtons.OK, MessageBoxIcon.Information);
                        testButton.Enabled = true;
                    });
                    log("test-ok agent=" + agentName);
                }
                catch (Exception ex)
                {
                    InvokeUi(delegate
                    {
                        outputBox.Text = AppendLine(outputBox.Text, "Test failed: " + ex.Message);
                        MessageBox.Show(this, ex.Message, "Qexow CAM test failed", MessageBoxButtons.OK, MessageBoxIcon.Error);
                        testButton.Enabled = true;
                    });
                    log("test-error agent=" + agentName + " error=" + ex.Message);
                }
            });
        }

        private List<Dictionary<string, object>> LoadAgents()
        {
            try
            {
                Dictionary<string, object> result = ApiGet("/agents");
                if (result.ContainsKey("agents") && result["agents"] is ArrayList)
                {
                    List<Dictionary<string, object>> apiAgents = ConvertAgentArray((ArrayList)result["agents"]);
                    if (showArchivedCheckBox != null && showArchivedCheckBox.Checked)
                    {
                        log("active-filter bypass showArchived=true source=api count=" + apiAgents.Count);
                        return apiAgents;
                    }
                    return FilterActiveAgents(apiAgents);
                }
            }
            catch
            {
            }

            string registryPath = Path.Combine(CamPaths.Root, "agents.json");
            string text = File.ReadAllText(registryPath);
            Dictionary<string, object> registry = json.Deserialize<Dictionary<string, object>>(text);
            ArrayList rows = new ArrayList();
            if (registry.ContainsKey("agents") && registry["agents"] is Dictionary<string, object>)
            {
                foreach (object value in ((Dictionary<string, object>)registry["agents"]).Values)
                {
                    rows.Add(value);
                }
            }
            List<Dictionary<string, object>> agents = ConvertAgentArray(rows);
            if (showArchivedCheckBox != null && showArchivedCheckBox.Checked)
            {
                log("active-filter bypass showArchived=true count=" + agents.Count);
                return agents;
            }
            return FilterActiveAgents(agents);
        }

        private List<Dictionary<string, object>> ConvertAgentArray(ArrayList rows)
        {
            List<Dictionary<string, object>> agents = new List<Dictionary<string, object>>();
            foreach (object row in rows)
            {
                Dictionary<string, object> agent = row as Dictionary<string, object>;
                if (agent != null) agents.Add(agent);
            }
            agents.Sort(delegate(Dictionary<string, object> a, Dictionary<string, object> b)
            {
                return String.Compare(Value(a, "name"), Value(b, "name"), StringComparison.OrdinalIgnoreCase);
            });
            return agents;
        }

        private List<Dictionary<string, object>> FilterActiveAgents(List<Dictionary<string, object>> agents)
        {
            HashSet<string> activeThreadIds = LoadActiveThreadIds();
            if (activeThreadIds.Count == 0)
            {
                log("active-filter skipped reason=no-active-thread-ids");
                return agents;
            }

            List<Dictionary<string, object>> filtered = new List<Dictionary<string, object>>();
            foreach (Dictionary<string, object> agent in agents)
            {
                string threadId = Value(agent, "threadId");
                if (!String.IsNullOrWhiteSpace(threadId) && activeThreadIds.Contains(threadId))
                {
                    if (activeThreadMetadata.ContainsKey(threadId))
                    {
                        Dictionary<string, object> thread = activeThreadMetadata[threadId];
                        agent["chatTitle"] = Value(thread, "title");
                        agent["threadSource"] = Value(thread, "thread_source");
                    }
                    filtered.Add(agent);
                }
            }
            log("active-filter applied active=" + filtered.Count + " total=" + agents.Count + " hidden=" + (agents.Count - filtered.Count));
            return filtered;
        }

        private HashSet<string> LoadActiveThreadIds()
        {
            HashSet<string> ids = LoadActiveThreadIdsFromClassifier();
            if (ids.Count > 0) return ids;
            log("active-filter failed reason=robust-classifier-unavailable");
            return ids;
        }

        private HashSet<string> LoadActiveThreadIdsFromClassifier()
        {
            HashSet<string> ids = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            Dictionary<string, Dictionary<string, object>> metadata = new Dictionary<string, Dictionary<string, object>>(StringComparer.OrdinalIgnoreCase);
            string script = FindQueryThreadsScript();
            if (String.IsNullOrWhiteSpace(script))
            {
                log("active-classifier-skipped reason=query_threads.py-not-found");
                return ids;
            }

            try
            {
                ProcessStartInfo psi = new ProcessStartInfo();
                psi.FileName = "python";
                psi.Arguments = "\"" + script.Replace("\"", "\\\"") + "\"";
                psi.UseShellExecute = false;
                psi.CreateNoWindow = true;
                psi.RedirectStandardOutput = true;
                psi.RedirectStandardError = true;
                using (Process process = Process.Start(psi))
                {
                    string output = process.StandardOutput.ReadToEnd();
                    string error = process.StandardError.ReadToEnd();
                    process.WaitForExit(10000);
                    if (!process.HasExited)
                    {
                        try { process.Kill(); } catch {}
                        log("active-classifier-error timeout");
                        return ids;
                    }
                    if (process.ExitCode != 0)
                    {
                        log("active-classifier-error " + error.Trim());
                        return ids;
                    }

                    Dictionary<string, object> result = json.Deserialize<Dictionary<string, object>>(output);
                    if (result != null && result.ContainsKey("threads") && result["threads"] is ArrayList)
                    {
                        ArrayList threads = (ArrayList)result["threads"];
                        foreach (object row in threads)
                        {
                            Dictionary<string, object> thread = row as Dictionary<string, object>;
                            if (thread == null) continue;
                            string id = Value(thread, "id");
                            if (!String.IsNullOrWhiteSpace(id))
                            {
                                ids.Add(id);
                                metadata[id] = thread;
                            }
                        }
                        activeThreadMetadata = metadata;
                        log("active-classifier-loaded count=" + ids.Count + " script=\"" + script + "\"");
                    }
                }
            }
            catch (Exception ex)
            {
                log("active-classifier-exception " + ex.Message);
            }
            return ids;
        }

        private string FindQueryThreadsScript()
        {
            string exeDir = AppDomain.CurrentDomain.BaseDirectory;
            string[] candidates = new string[]
            {
                Path.Combine(exeDir, "query_threads.py"),
                Path.Combine(exeDir, "src", "query_threads.py"),
                Path.Combine(Environment.CurrentDirectory, "src", "query_threads.py"),
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "OneDrive", "Documents", "New project", "codex-agent-manager", "src", "query_threads.py")
            };
            foreach (string candidate in candidates)
            {
                try
                {
                    if (File.Exists(candidate)) return candidate;
                }
                catch {}
            }
            return null;
        }

        private HashSet<string> LoadActiveThreadIdsFromSqlite()
        {
            HashSet<string> ids = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            string db = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".codex", "state_5.sqlite");
            if (!File.Exists(db)) return ids;

            try
            {
                ProcessStartInfo psi = new ProcessStartInfo();
                psi.FileName = "python";
                psi.Arguments = "-c \"import sqlite3,sys; db=sys.argv[1]; con=sqlite3.connect(db); [print(r[0]) for r in con.execute('select id from threads where archived=0')]\" \"" + db.Replace("\"", "\\\"") + "\"";
                psi.UseShellExecute = false;
                psi.CreateNoWindow = true;
                psi.RedirectStandardOutput = true;
                psi.RedirectStandardError = true;
                using (Process process = Process.Start(psi))
                {
                    string output = process.StandardOutput.ReadToEnd();
                    string error = process.StandardError.ReadToEnd();
                    process.WaitForExit(5000);
                    if (process.ExitCode != 0)
                    {
                        log("active-sqlite-error " + error.Trim());
                        return ids;
                    }
                    foreach (string line in output.Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries))
                    {
                        ids.Add(line.Trim());
                    }
                }
                log("active-sqlite-loaded count=" + ids.Count);
            }
            catch (Exception ex)
            {
                log("active-sqlite-exception " + ex.Message);
            }
            return ids;
        }

        private HashSet<string> LoadActiveThreadIdsFromSessionFiles()
        {
            HashSet<string> ids = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            string sessions = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".codex", "sessions");
            if (!Directory.Exists(sessions)) return ids;
            try
            {
                foreach (string file in Directory.GetFiles(sessions, "*.jsonl", SearchOption.AllDirectories))
                {
                    string name = Path.GetFileNameWithoutExtension(file);
                    int idx = name.LastIndexOf('-');
                    if (idx >= 0 && idx + 1 < name.Length)
                    {
                        string id = name.Substring(idx + 1);
                        if (id.Length >= 30) ids.Add(id);
                    }
                }
                log("active-session-files-loaded count=" + ids.Count);
            }
            catch (Exception ex)
            {
                log("active-session-files-error " + ex.Message);
            }
            return ids;
        }

        private Dictionary<string, object> ApiGet(string path)
        {
            return ApiRequest("GET", path, null);
        }

        private Dictionary<string, object> ApiPost(string path, Dictionary<string, object> body)
        {
            return ApiRequest("POST", path, json.Serialize(body));
        }

        private Dictionary<string, object> ApiRequest(string method, string path, string body)
        {
            int port = CamPaths.Port;
            HttpWebRequest request = (HttpWebRequest)WebRequest.Create("http://127.0.0.1:" + port + path);
            request.Method = method;
            request.Headers["Authorization"] = "Bearer " + CamPaths.LocalToken;
            request.Timeout = 15000;
            if (body != null)
            {
                byte[] data = Encoding.UTF8.GetBytes(body);
                request.ContentType = "application/json";
                request.ContentLength = data.Length;
                using (Stream stream = request.GetRequestStream()) stream.Write(data, 0, data.Length);
            }
            using (HttpWebResponse response = (HttpWebResponse)request.GetResponse())
            using (StreamReader reader = new StreamReader(response.GetResponseStream()))
            {
                string text = reader.ReadToEnd();
                if (String.IsNullOrWhiteSpace(text)) return new Dictionary<string, object>();
                return json.Deserialize<Dictionary<string, object>>(text);
            }
        }

        private void InvokeUi(Action action)
        {
            if (IsDisposed) return;
            if (InvokeRequired) BeginInvoke(action);
            else action();
        }

        private static string Value(Dictionary<string, object> map, string key)
        {
            if (map == null || !map.ContainsKey(key) || map[key] == null) return "";
            return Convert.ToString(map[key]);
        }

        private static string DisplayTitle(Dictionary<string, object> agent)
        {
            string title = Value(agent, "chatTitle");
            if (!String.IsNullOrWhiteSpace(title)) return title;
            string name = Value(agent, "name");
            if (!String.IsNullOrWhiteSpace(name)) return name;
            return Value(agent, "threadId");
        }

        private bool TryStartDaemon()
        {
            daemonStartAttempted = true;
            try
            {
                string exeDir = AppDomain.CurrentDomain.BaseDirectory;
                string camExe = Path.Combine(exeDir, "cam.exe");
                if (!File.Exists(camExe))
                {
                    string repoCamExe = Path.Combine(Environment.CurrentDirectory, "dist", "cam.exe");
                    if (File.Exists(repoCamExe)) camExe = repoCamExe;
                }
                if (!File.Exists(camExe))
                {
                    log("daemon-start-skipped reason=cam.exe-not-found");
                    return false;
                }

                ProcessStartInfo psi = new ProcessStartInfo();
                psi.FileName = camExe;
                psi.Arguments = "daemon start";
                psi.WorkingDirectory = Path.GetDirectoryName(camExe);
                psi.UseShellExecute = false;
                psi.CreateNoWindow = true;
                psi.WindowStyle = ProcessWindowStyle.Hidden;
                Process process = Process.Start(psi);
                log("daemon-started-from-gui pid=" + (process == null ? "unknown" : Convert.ToString(process.Id)) + " exe=\"" + camExe + "\"");
                return true;
            }
            catch (Exception ex)
            {
                log("daemon-start-error " + ex.Message);
                return false;
            }
        }

        private static string AppendLine(string existing, string line)
        {
            if (String.IsNullOrEmpty(existing)) return DateTime.Now.ToString("T") + "  " + line;
            return existing + Environment.NewLine + DateTime.Now.ToString("T") + "  " + line;
        }
    }

    static class CamPaths
    {
        public static string Root
        {
            get
            {
                string env = Environment.GetEnvironmentVariable("CAM_HOME");
                if (!String.IsNullOrWhiteSpace(env)) return env;
                return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".qexow-cam");
            }
        }

        public static string LocalToken
        {
            get
            {
                return File.ReadAllText(Path.Combine(Root, "secrets", "local-api-token")).Trim();
            }
        }

        public static int Port
        {
            get
            {
                try
                {
                    string config = File.ReadAllText(Path.Combine(Root, "config.json"));
                    int idx = config.IndexOf("\"port\"", StringComparison.OrdinalIgnoreCase);
                    if (idx >= 0)
                    {
                        int colon = config.IndexOf(':', idx);
                        if (colon >= 0)
                        {
                            int end = colon + 1;
                            while (end < config.Length && Char.IsWhiteSpace(config[end])) end++;
                            int start = end;
                            while (end < config.Length && Char.IsDigit(config[end])) end++;
                            int parsed;
                            if (Int32.TryParse(config.Substring(start, end - start), out parsed)) return parsed;
                        }
                    }
                }
                catch
                {
                }
                return 37631;
            }
        }
    }
}
