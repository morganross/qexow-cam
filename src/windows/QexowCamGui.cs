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
using System.Reflection;
using System.Net.Sockets;

[assembly: AssemblyVersion("2.1.49.0")]
[assembly: AssemblyFileVersion("2.1.49.0")]
[assembly: AssemblyInformationalVersion("2.1.49")]

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
        private readonly Label overviewLabel;
        private readonly Label discoveryLabel;
        private readonly Panel daemonLight;
        private readonly DataGridView peersGrid;
        private readonly DataGridView agentsGrid;
        private readonly TextBox outputBox;
        private readonly Button refreshButton;
        private readonly Button testButton;
        private readonly CheckBox showArchivedCheckBox;
        private readonly System.Windows.Forms.Timer autoRefreshTimer;
        private readonly JavaScriptSerializer json = new JavaScriptSerializer();
        private const string CamTestMailboxAgent = "CAM test, Kexau CAM test suite mailbox";
        private Dictionary<string, Dictionary<string, object>> activeThreadMetadata = new Dictionary<string, Dictionary<string, object>>(StringComparer.OrdinalIgnoreCase);
        private readonly object daemonStartLock = new object();
        private readonly object refreshStateLock = new object();
        private bool daemonStartAttempted = false;
        private bool daemonStartInProgress = false;
        private bool daemonRecoveryAttempted = false;
        private bool refreshInProgress = false;
        private bool refreshPending = false;

        public MainForm(Action<string> logger)
        {
            log = logger;
            Text = "Qexow CAM Status v" + CamPaths.Version;
            StartPosition = FormStartPosition.CenterScreen;
            Size = new Size(1100, 720);
            MinimumSize = new Size(840, 560);
            Font = new Font("Segoe UI", 9.0f);

            TableLayoutPanel root = new TableLayoutPanel();
            root.Dock = DockStyle.Fill;
            root.RowCount = 6;
            root.ColumnCount = 1;
            root.RowStyles.Add(new RowStyle(SizeType.Absolute, 72));
            root.RowStyles.Add(new RowStyle(SizeType.Absolute, 54));
            root.RowStyles.Add(new RowStyle(SizeType.Percent, 28));
            root.RowStyles.Add(new RowStyle(SizeType.Percent, 32));
            root.RowStyles.Add(new RowStyle(SizeType.Absolute, 48));
            root.RowStyles.Add(new RowStyle(SizeType.Percent, 40));
            Controls.Add(root);

            Panel header = new Panel();
            header.Dock = DockStyle.Fill;
            header.Padding = new Padding(16, 12, 16, 8);
            header.BackColor = Color.FromArgb(245, 247, 250);
            root.Controls.Add(header, 0, 0);

            Label title = new Label();
            title.Text = "Qexow CAM v" + CamPaths.Version;
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

            Panel summaryPanel = new Panel();
            summaryPanel.Dock = DockStyle.Fill;
            summaryPanel.Padding = new Padding(16, 8, 16, 4);
            root.Controls.Add(summaryPanel, 0, 1);

            overviewLabel = new Label();
            overviewLabel.Text = "Mappings summary pending...";
            overviewLabel.AutoSize = true;
            overviewLabel.Location = new Point(16, 6);
            summaryPanel.Controls.Add(overviewLabel);

            discoveryLabel = new Label();
            discoveryLabel.Text = "Remote discovery summary pending...";
            discoveryLabel.AutoSize = true;
            discoveryLabel.Location = new Point(16, 28);
            summaryPanel.Controls.Add(discoveryLabel);

            peersGrid = new DataGridView();
            peersGrid.Dock = DockStyle.Fill;
            peersGrid.ReadOnly = true;
            peersGrid.AllowUserToAddRows = false;
            peersGrid.AllowUserToDeleteRows = false;
            peersGrid.SelectionMode = DataGridViewSelectionMode.FullRowSelect;
            peersGrid.MultiSelect = false;
            peersGrid.AutoSizeColumnsMode = DataGridViewAutoSizeColumnsMode.Fill;
            peersGrid.RowHeadersVisible = false;
            root.Controls.Add(peersGrid, 0, 2);

            agentsGrid = new DataGridView();
            agentsGrid.Dock = DockStyle.Fill;
            agentsGrid.ReadOnly = true;
            agentsGrid.AllowUserToAddRows = false;
            agentsGrid.AllowUserToDeleteRows = false;
            agentsGrid.SelectionMode = DataGridViewSelectionMode.FullRowSelect;
            agentsGrid.MultiSelect = false;
            agentsGrid.AutoSizeColumnsMode = DataGridViewAutoSizeColumnsMode.Fill;
            agentsGrid.RowHeadersVisible = false;
            root.Controls.Add(agentsGrid, 0, 3);

            Panel buttons = new Panel();
            buttons.Dock = DockStyle.Fill;
            buttons.Padding = new Padding(10, 8, 10, 8);
            root.Controls.Add(buttons, 0, 4);

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
            root.Controls.Add(outputBox, 0, 5);

            autoRefreshTimer = new System.Windows.Forms.Timer();
            autoRefreshTimer.Interval = 15000;
            autoRefreshTimer.Tick += delegate { RefreshAll(true); };
            autoRefreshTimer.Start();
        }

        public void RefreshAll()
        {
            RefreshAll(false);
        }

        private void RefreshAll(bool automatic)
        {
            lock (refreshStateLock)
            {
                if (refreshInProgress)
                {
                    refreshPending = true;
                    log("refresh-skip already-in-progress automatic=" + automatic);
                    return;
                }
                refreshInProgress = true;
                refreshPending = false;
            }

            log("refresh-start automatic=" + automatic);
            if (!automatic)
            {
                outputBox.Text = AppendLine(outputBox.Text, "Refreshing CAM status...");
            }
            ThreadPool.QueueUserWorkItem(delegate
            {
                bool healthOk = false;
                try
                {
                    Dictionary<string, object> health = ApiGet("/health");
                    object nodeName = health.ContainsKey("nodeName") ? health["nodeName"] : "";
                    object version = health.ContainsKey("version") ? health["version"] : CamPaths.Version;
                    object startedAt = health.ContainsKey("startedAt") ? health["startedAt"] : "";
                    InvokeUi(delegate
                    {
                        daemonLight.BackColor = Color.LimeGreen;
                        daemonLabel.Text = "Daemon online - version=" + version + " node=" + nodeName + " started=" + startedAt;
                    });
                    log("health-ok");
                    healthOk = true;
                }
                catch (Exception ex)
                {
                    if (TryStartDaemon())
                    {
                        Thread.Sleep(2000);
                        try
                        {
                            Dictionary<string, object> retryHealth = ApiGet("/health");
                            object retryNodeName = retryHealth.ContainsKey("nodeName") ? retryHealth["nodeName"] : "";
                            object retryVersion = retryHealth.ContainsKey("version") ? retryHealth["version"] : CamPaths.Version;
                            object retryStartedAt = retryHealth.ContainsKey("startedAt") ? retryHealth["startedAt"] : "";
                            InvokeUi(delegate
                            {
                                daemonLight.BackColor = Color.LimeGreen;
                                daemonLabel.Text = "Daemon online - version=" + retryVersion + " node=" + retryNodeName + " started=" + retryStartedAt;
                            });
                            log("health-ok-after-daemon-start");
                            healthOk = true;
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
                    List<Dictionary<string, object>> peers = LoadPeers();
                    InvokeUi(delegate
                    {
                        RenderPeers(peers);
                        RenderPeerSummary(peers, !automatic);
                    });

                    List<Dictionary<string, object>> agents = LoadAgents();
                    InvokeUi(delegate
                    {
                        RenderAgents(agents);
                        RenderAgentSummary(agents, !automatic);
                    });
                    log("agents-loaded count=" + agents.Count);
                }
                catch (Exception ex)
                {
                    if (healthOk && IsUnauthorized(ex) && RecoverFromForeignDaemon())
                    {
                        InvokeUi(delegate
                        {
                            outputBox.Text = AppendLine(outputBox.Text, "Detected a stale/foreign daemon on the CAM port. Replacing it with the installed daemon and retrying...");
                        });
                        log("daemon-recovery-retrying-after-unauthorized");
                        RefreshAll();
                        return;
                    }
                    InvokeUi(delegate { outputBox.Text = AppendLine(outputBox.Text, "Agent load error: " + ex.Message); });
                    log("agents-error " + ex.Message);
                }
                finally
                {
                    bool rerun;
                    lock (refreshStateLock)
                    {
                        refreshInProgress = false;
                        rerun = refreshPending;
                        refreshPending = false;
                    }
                    if (rerun)
                    {
                        BeginInvoke((Action)delegate { RefreshAll(true); });
                    }
                }
            });
        }

        private void RenderAgents(List<Dictionary<string, object>> agents)
        {
            agentsGrid.Columns.Clear();
            agentsGrid.Rows.Clear();
            foreach (string column in new[] { "light", "chatTitle", "name", "status", "node", "route", "source", "testable", "threadId", "activeTurnId", "cwd", "model" })
            {
                agentsGrid.Columns.Add(column, column);
            }
            agentsGrid.Columns["light"].HeaderText = "";
            agentsGrid.Columns["chatTitle"].HeaderText = "chat";
            agentsGrid.Columns["name"].HeaderText = "agent mapping";
            agentsGrid.Columns["route"].HeaderText = "route";
            agentsGrid.Columns["source"].HeaderText = "source";
            agentsGrid.Columns["light"].Width = 34;
            agentsGrid.Columns["light"].FillWeight = 8;
            agentsGrid.Columns["chatTitle"].FillWeight = 28;
            agentsGrid.Columns["name"].FillWeight = 22;
            agentsGrid.Columns["route"].FillWeight = 12;
            agentsGrid.Columns["source"].FillWeight = 12;
            agentsGrid.Columns["testable"].FillWeight = 8;
            foreach (Dictionary<string, object> agent in agents)
            {
                int rowIndex = agentsGrid.Rows.Add(
                    "●",
                    DisplayTitle(agent),
                    Value(agent, "name"),
                    Value(agent, "status"),
                    Value(agent, "node"),
                    Value(agent, "route"),
                    Value(agent, "sourceHost"),
                    IsAgentTestable(agent) ? "yes" : "no",
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
            if (agents.Count == 0)
            {
                outputBox.Text = AppendLine(outputBox.Text, "Refreshing CAM status; discovery has not returned testable mappings yet.");
            }
        }

        private void RenderPeers(List<Dictionary<string, object>> peers)
        {
            peersGrid.Columns.Clear();
            peersGrid.Rows.Clear();
            foreach (string column in new[] { "ok", "name", "state", "transport", "ssh", "candidateIps", "candidateUsers", "key", "mirrored", "raw", "approved", "quarantined", "rejected", "schema", "remoteNode", "syncedAt", "blocker" })
            {
                peersGrid.Columns.Add(column, column);
            }
            peersGrid.Columns["ok"].HeaderText = "ok";
            peersGrid.Columns["name"].HeaderText = "peer";
            peersGrid.Columns["state"].HeaderText = "state";
            peersGrid.Columns["candidateIps"].HeaderText = "candidate IPs";
            peersGrid.Columns["candidateUsers"].HeaderText = "users";
            peersGrid.Columns["key"].HeaderText = "key";
            peersGrid.Columns["mirrored"].HeaderText = "mirrored";
            peersGrid.Columns["raw"].HeaderText = "raw";
            peersGrid.Columns["approved"].HeaderText = "approved";
            peersGrid.Columns["quarantined"].HeaderText = "quarantined";
            peersGrid.Columns["rejected"].HeaderText = "rejected";
            peersGrid.Columns["schema"].HeaderText = "schema";
            peersGrid.Columns["remoteNode"].HeaderText = "remote node";
            peersGrid.Columns["syncedAt"].HeaderText = "last sync";
            peersGrid.Columns["blocker"].HeaderText = "blocker";
            peersGrid.Columns["ok"].FillWeight = 5;
            peersGrid.Columns["name"].FillWeight = 16;
            peersGrid.Columns["state"].FillWeight = 10;
            peersGrid.Columns["transport"].FillWeight = 9;
            peersGrid.Columns["ssh"].FillWeight = 16;
            peersGrid.Columns["candidateIps"].FillWeight = 14;
            peersGrid.Columns["candidateUsers"].FillWeight = 8;
            peersGrid.Columns["key"].FillWeight = 7;
            peersGrid.Columns["mirrored"].FillWeight = 6;
            peersGrid.Columns["raw"].FillWeight = 6;
            peersGrid.Columns["approved"].FillWeight = 6;
            peersGrid.Columns["quarantined"].FillWeight = 6;
            peersGrid.Columns["rejected"].FillWeight = 6;
            peersGrid.Columns["schema"].FillWeight = 6;
            peersGrid.Columns["remoteNode"].FillWeight = 10;
            peersGrid.Columns["syncedAt"].FillWeight = 10;
            peersGrid.Columns["blocker"].FillWeight = 24;

            foreach (Dictionary<string, object> peer in peers)
            {
                string state = Value(peer, "state").ToLowerInvariant();
                bool isOk = state == "mirrored" || state == "mirrored-degraded" || state == "verified" || state == "probe-ready";
                int rowIndex = peersGrid.Rows.Add(
                    isOk ? "yes" : "no",
                    Value(peer, "name"),
                    Value(peer, "state"),
                    Value(peer, "transport"),
                    Value(peer, "ssh"),
                    JoinList(peer, "candidateIps"),
                    JoinList(peer, "candidateUsernames"),
                    String.IsNullOrWhiteSpace(Value(peer, "key")) ? "no" : "yes",
                    Value(peer, "mirroredAgents"),
                    Value(peer, "remoteRawDiscoveries"),
                    Value(peer, "remoteApprovedDiscoveries"),
                    Value(peer, "remoteQuarantinedDiscoveries"),
                    Value(peer, "remoteRejectedDiscoveries"),
                    Value(peer, "remoteInventorySchema") + (ValueBool(peer, "remoteInventoryDegraded") ? " legacy" : ""),
                    Value(peer, "remoteNodeName"),
                    ShortIso(Value(peer, "syncedAt")),
                    Value(peer, "blockerSummary")
                );
                DataGridViewRow row = peersGrid.Rows[rowIndex];
                Color color = Color.Gray;
                if (state == "mirrored") color = Color.LimeGreen;
                else if (state == "mirrored-degraded") color = Color.Goldenrod;
                else if (state == "verified" || state == "probe-ready") color = Color.DodgerBlue;
                else if (state == "missing-key" || state == "missing-ip" || state == "missing-username" || state == "sync-failed" || state == "probe-failed") color = Color.OrangeRed;
                row.DefaultCellStyle.ForeColor = color;
            }
        }

        private void RenderAgentSummary(List<Dictionary<string, object>> agents, bool emitOutputLine)
        {
            int testableCount = 0;
            int remoteCount = 0;
            foreach (Dictionary<string, object> agent in agents)
            {
                if (IsAgentTestable(agent)) testableCount++;
                string route = Value(agent, "route");
                if (!String.IsNullOrWhiteSpace(route) && !String.Equals(route, "local", StringComparison.OrdinalIgnoreCase)) remoteCount++;
            }
            overviewLabel.Text = "Agent mappings: " + agents.Count + " total, " + testableCount + " active/testable, " + remoteCount + " remote, " + (agents.Count - testableCount) + " skipped/limited.";
            if (emitOutputLine && agents.Count > 0)
            {
                outputBox.Text = AppendLine(outputBox.Text, "Loaded " + testableCount + " active/testable, " + remoteCount + " remote, " + (agents.Count - testableCount) + " skipped/limited agent/session mappings.");
            }
        }

        private void RenderPeerSummary(List<Dictionary<string, object>> peers, bool emitOutputLine)
        {
            int ok = 0;
            int mirrored = 0;
            int missingKey = 0;
            int missingIp = 0;
            int missingUsername = 0;
            int probeReady = 0;
            int probeFailed = 0;
            int syncFailed = 0;
            int rawDiscoveries = 0;
            int approvedDiscoveries = 0;
            int rejectedDiscoveries = 0;
            foreach (Dictionary<string, object> peer in peers)
            {
                string state = Value(peer, "state");
                if (String.Equals(state, "mirrored", StringComparison.OrdinalIgnoreCase) ||
                    String.Equals(state, "mirrored-degraded", StringComparison.OrdinalIgnoreCase) ||
                    String.Equals(state, "verified", StringComparison.OrdinalIgnoreCase) ||
                    String.Equals(state, "probe-ready", StringComparison.OrdinalIgnoreCase))
                {
                    ok++;
                }
                if (String.Equals(state, "mirrored", StringComparison.OrdinalIgnoreCase)) mirrored++;
                if (String.Equals(state, "mirrored-degraded", StringComparison.OrdinalIgnoreCase)) mirrored++;
                if (String.Equals(state, "missing-key", StringComparison.OrdinalIgnoreCase)) missingKey++;
                if (String.Equals(state, "missing-ip", StringComparison.OrdinalIgnoreCase)) missingIp++;
                if (String.Equals(state, "missing-username", StringComparison.OrdinalIgnoreCase)) missingUsername++;
                if (String.Equals(state, "probe-ready", StringComparison.OrdinalIgnoreCase)) probeReady++;
                if (String.Equals(state, "probe-failed", StringComparison.OrdinalIgnoreCase)) probeFailed++;
                if (String.Equals(state, "sync-failed", StringComparison.OrdinalIgnoreCase)) syncFailed++;
                rawDiscoveries += ToInt(Value(peer, "remoteRawDiscoveries"));
                approvedDiscoveries += ToInt(Value(peer, "remoteApprovedDiscoveries"));
                rejectedDiscoveries += ToInt(Value(peer, "remoteQuarantinedDiscoveries")) + ToInt(Value(peer, "remoteRejectedDiscoveries"));
            }
            discoveryLabel.Text = "Remote discovery: " + peers.Count + " peers, " + ok + " OK, " + mirrored + " mirrored, " + rawDiscoveries + " raw, " + approvedDiscoveries + " approved, " + rejectedDiscoveries + " not promoted, " + probeReady + " ready, " + probeFailed + " probe failed, " + syncFailed + " sync failed.";
            if (emitOutputLine)
            {
                outputBox.Text = AppendLine(outputBox.Text, "Remote discovery loaded " + peers.Count + " peer rows; " + ok + " OK.");
            }
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
            string threadId = Convert.ToString(agentsGrid.SelectedRows[0].Cells["threadId"].Value);
            string status = Convert.ToString(agentsGrid.SelectedRows[0].Cells["status"].Value);
            string testable = Convert.ToString(agentsGrid.SelectedRows[0].Cells["testable"].Value);
            if (String.IsNullOrWhiteSpace(threadId) || String.Equals(status, "stale", StringComparison.OrdinalIgnoreCase) || String.Equals(status, "unbound", StringComparison.OrdinalIgnoreCase) || !String.Equals(testable, "yes", StringComparison.OrdinalIgnoreCase))
            {
                AppendOutput("TEST FAIL  selected agent is not testable: status=" + status + " threadId=" + threadId + " testable=" + testable);
                return;
            }

            testButton.Enabled = false;
            testButton.Text = "Testing...";
            AppendOutput("TEST START  target=" + agentName);
            AppendOutput("STATE send  sending test message...");
            log("test-start agent=" + agentName);

            ThreadPool.QueueUserWorkItem(delegate
            {
                try
                {
                    DateTime testStartUtc = DateTime.UtcNow;
                    string correlationId = Guid.NewGuid().ToString("N");
                    Dictionary<string, object> payload = new Dictionary<string, object>();
                    payload["targetAgent"] = agentName;
                    payload["sourceAgent"] = CamTestMailboxAgent;
                    payload["sourceNode"] = Environment.MachineName;
                    payload["correlationId"] = correlationId;
                    payload["strict"] = true;
                    payload["messageType"] = "cam-gui-test";
                    payload["message"] = "Hello, how is your day? Respond by sending a Qexow CAM reply to targetAgent \"" + CamTestMailboxAgent + "\" with correlationId \"" + correlationId + "\" and messageType \"cam-gui-test-reply\". Your reply body must include CAM_GUI_TEST_RESPONSE " + correlationId + ", your agent name, node name, current status, and the answer to this question: what is the capital of Missouri? Do not only answer in this chat.";

                    Dictionary<string, object> sendResult = ApiPost("/send", payload);
                    ValidateStrictSend(sendResult);
                    string turnId = NestedValue(sendResult, "message", "turnId");
                    InvokeUi(delegate
                    {
                        AppendOutput("STATE outbound-delivered  message entered selected agent thread");
                        AppendOutput("STATE waiting-for-reply  turnId=" + turnId + " testId=" + correlationId);
                        AppendOutput("STATE waiting-for-reply  waiting for CAM receiver reply to " + CamTestMailboxAgent + ", not reading session logs");
                    });

                    string response = WaitForMailboxResponse(agentName, correlationId, testStartUtc, 90000);
                    InvokeUi(delegate
                    {
                        bool failedReply = response.StartsWith("TEST_FAIL:", StringComparison.Ordinal);
                        if (failedReply)
                        {
                            AppendOutput(response.Substring("TEST_FAIL:".Length));
                            AppendOutput("TEST FAIL");
                        }
                        else
                        {
                            MarkGuiTestPassed(correlationId, agentName);
                            AppendOutput("STATE reply-received  CAM receiver reply received from " + agentName);
                            AppendOutput(response);
                            AppendOutput("TEST PASS");
                        }
                        testButton.Enabled = true;
                        testButton.Text = "Test Selected Agent";
                    });
                    log((response.StartsWith("TEST_FAIL:", StringComparison.Ordinal) ? "test-fail" : "test-ok") + " agent=" + agentName);
                }
                catch (Exception ex)
                {
                    InvokeUi(delegate
                    {
                        AppendOutput("STATE fail  " + ex.Message);
                        AppendOutput("TEST FAIL");
                        testButton.Enabled = true;
                        testButton.Text = "Test Selected Agent";
                    });
                    log("test-error agent=" + agentName + " error=" + ex.Message);
                }
            });
        }

        private string WaitForMailboxResponse(string agentName, string correlationId, DateTime testStartUtc, int timeoutMs)
        {
            Stopwatch stopwatch = Stopwatch.StartNew();
            string lastSummary = "no matching CAM inbox reply seen";
            int pollCount = 0;
            string[] spinner = new string[] { "-", "\\", "|", "/" };
            while (stopwatch.ElapsedMilliseconds < timeoutMs)
            {
                pollCount++;
                try
                {
                    int elapsedSeconds = (int)(stopwatch.ElapsedMilliseconds / 1000);
                    string mark = spinner[pollCount % spinner.Length];
                    InvokeUi(delegate
                    {
                        AppendOutput("STATE poll " + mark + " elapsed=" + elapsedSeconds + "s reading " + CamTestMailboxAgent + " CAM inbox...");
                    });

                    Dictionary<string, object> inboxResult = ApiGet("/inbox?agent=" + Uri.EscapeDataString(CamTestMailboxAgent) + "&wait=5");
                    string response = FindMailboxResponse(inboxResult, agentName, correlationId, testStartUtc);
                    if (!String.IsNullOrWhiteSpace(response)) return response;
                    lastSummary = SummarizeInbox(inboxResult, agentName, correlationId);
                    InvokeUi(delegate
                    {
                        AppendOutput("STATE seen  no matching reply yet; " + lastSummary);
                    });
                }
                catch (Exception ex)
                {
                    lastSummary = "read error: " + ex.Message;
                    InvokeUi(delegate
                    {
                        AppendOutput("STATE read-error  " + ex.Message);
                    });
                }
                Thread.Sleep(2000);
            }

            throw new Exception("Timed out waiting for a CAM inbox reply from " + agentName + " with testId=" + correlationId + ". " + lastSummary);
        }

        private string FindMailboxResponse(Dictionary<string, object> inboxResult, string expectedAgentName, string correlationId, DateTime testStartUtc)
        {
            ArrayList messages = inboxResult != null && inboxResult.ContainsKey("messages") ? inboxResult["messages"] as ArrayList : null;
            if (messages == null) return "";
            for (int i = messages.Count - 1; i >= 0; i--)
            {
                Dictionary<string, object> message = messages[i] as Dictionary<string, object>;
                if (message == null) continue;
                string body = Value(message, "body");
                string messageCorrelationId = Value(message, "correlationId");
                string sourceAgent = Value(message, "sourceAgent");
                string targetAgent = Value(message, "targetAgent");
                string messageType = Value(message, "messageType");
                string delivery = Value(message, "delivery");
                if (!IsCurrentTestTimestamp(Value(message, "timestamp"), testStartUtc))
                {
                    continue;
                }
                bool idMatches = String.Equals(messageCorrelationId, correlationId, StringComparison.OrdinalIgnoreCase) ||
                    body.IndexOf(correlationId, StringComparison.OrdinalIgnoreCase) >= 0;
                if (!idMatches) continue;
                if (!String.Equals(sourceAgent, expectedAgentName, StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }
                if (!String.Equals(targetAgent, CamTestMailboxAgent, StringComparison.Ordinal))
                {
                    continue;
                }
                if (!String.IsNullOrWhiteSpace(messageType) && !String.Equals(messageType, "cam-gui-test-reply", StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }
                if (body.IndexOf("CAM_GUI_TEST_RESPONSE", StringComparison.OrdinalIgnoreCase) < 0)
                {
                    continue;
                }
                if (!BodyContainsMissouriAnswer(body))
                {
                    return "TEST_FAIL:STATE semantic-fail  matched reply did not contain Jefferson/Jefferson City/city\r\n" + BuildReplyHeader(message, correlationId, messageCorrelationId, messageType, sourceAgent, targetAgent, delivery) + body;
                }
                if (String.IsNullOrWhiteSpace(body)) body = "(empty body)";
                string header = BuildReplyHeader(message, correlationId, messageCorrelationId, messageType, sourceAgent, targetAgent, delivery);
                if (!String.Equals(delivery, "received", StringComparison.OrdinalIgnoreCase))
                {
                    return "TEST_FAIL:STATE reply-queued-only  matched reply was not marked received\r\n" + header + body;
                }
                return header + body;
            }
            return "";
        }

        private bool BodyContainsMissouriAnswer(string body)
        {
            if (String.IsNullOrWhiteSpace(body)) return false;
            return body.IndexOf("Jefferson City", StringComparison.OrdinalIgnoreCase) >= 0 ||
                body.IndexOf("Jefferson", StringComparison.OrdinalIgnoreCase) >= 0 ||
                body.IndexOf("city", StringComparison.OrdinalIgnoreCase) >= 0;
        }

        private string BuildReplyHeader(Dictionary<string, object> message, string correlationId, string messageCorrelationId, string messageType, string sourceAgent, string targetAgent, string delivery)
        {
            return "CAM REPLY MATCHED\r\n" +
                "testId: " + correlationId + "\r\n" +
                "messageId: " + Value(message, "messageId") + "\r\n" +
                "correlationId: " + messageCorrelationId + "\r\n" +
                "messageType: " + messageType + "\r\n" +
                "sourceAgent: " + sourceAgent + "\r\n" +
                "sourceNode: " + Value(message, "sourceNode") + "\r\n" +
                "sourceRoute: " + Value(message, "sourceRoute") + "\r\n" +
                "targetAgent: " + targetAgent + "\r\n" +
                "delivery: " + delivery + "\r\n" +
                "error: " + Value(message, "error") + "\r\n" +
                "REPLY BODY:\r\n";
        }

        private void MarkGuiTestPassed(string correlationId, string agentName)
        {
            try
            {
                Dictionary<string, object> payload = new Dictionary<string, object>();
                payload["correlationId"] = correlationId;
                payload["agentName"] = agentName;
                payload["semanticCheck"] = "missouri-capital";
                ApiPost("/tests/pass", payload);
            }
            catch (Exception ex)
            {
                AppendOutput("STATE warn  failed to record passed test ledger: " + ex.Message);
            }
        }

        private bool IsCurrentTestTimestamp(string timestamp, DateTime testStartUtc)
        {
            if (String.IsNullOrWhiteSpace(timestamp)) return false;
            DateTime parsed;
            if (!DateTime.TryParse(timestamp, null, System.Globalization.DateTimeStyles.AdjustToUniversal, out parsed)) return false;
            return parsed.ToUniversalTime() >= testStartUtc.AddSeconds(-2);
        }

        private string SummarizeInbox(Dictionary<string, object> inboxResult, string expectedAgentName, string correlationId)
        {
            ArrayList messages = inboxResult != null && inboxResult.ContainsKey("messages") ? inboxResult["messages"] as ArrayList : null;
            if (messages == null) return CamTestMailboxAgent + " inbox unreadable";
            string mismatch = FindMismatchedMailboxResponse(messages, expectedAgentName, correlationId);
            if (!String.IsNullOrWhiteSpace(mismatch)) return CamTestMailboxAgent + " inbox messages=" + messages.Count + "; " + mismatch;
            return CamTestMailboxAgent + " inbox messages=" + messages.Count;
        }

        private string FindMismatchedMailboxResponse(ArrayList messages, string expectedAgentName, string correlationId)
        {
            for (int i = messages.Count - 1; i >= 0; i--)
            {
                Dictionary<string, object> message = messages[i] as Dictionary<string, object>;
                if (message == null) continue;
                string body = Value(message, "body");
                string messageCorrelationId = Value(message, "correlationId");
                bool idMatches = String.Equals(messageCorrelationId, correlationId, StringComparison.OrdinalIgnoreCase) ||
                    body.IndexOf(correlationId, StringComparison.OrdinalIgnoreCase) >= 0;
                if (!idMatches) continue;
                string sourceAgent = Value(message, "sourceAgent");
                if (!String.Equals(sourceAgent, expectedAgentName, StringComparison.OrdinalIgnoreCase))
                {
                    return "ignored same-testId reply from wrong sourceAgent=" + sourceAgent + "; expected=" + expectedAgentName;
                }
                if (Value(message, "targetAgent") != CamTestMailboxAgent)
                {
                    return "ignored same-testId reply to wrong targetAgent=" + Value(message, "targetAgent");
                }
                string messageType = Value(message, "messageType");
                if (!String.IsNullOrWhiteSpace(messageType) && !String.Equals(messageType, "cam-gui-test-reply", StringComparison.OrdinalIgnoreCase))
                {
                    return "ignored same-testId reply with wrong messageType=" + messageType;
                }
                if (body.IndexOf("CAM_GUI_TEST_RESPONSE", StringComparison.OrdinalIgnoreCase) < 0)
                {
                    return "ignored same-testId reply missing CAM_GUI_TEST_RESPONSE marker";
                }
            }
            return "";
        }

        private void ValidateStrictSend(Dictionary<string, object> sendResult)
        {
            if (sendResult == null) throw new Exception("Strict send failed: empty response");
            string ok = Value(sendResult, "ok");
            string delivered = Value(sendResult, "delivered");
            string queued = Value(sendResult, "queued");
            string error = Value(sendResult, "error");
            string messageError = NestedValue(sendResult, "message", "error");
            string turnId = NestedValue(sendResult, "message", "turnId");
            string delivery = NestedValue(sendResult, "message", "delivery");

            if (String.Equals(ok, "False", StringComparison.OrdinalIgnoreCase) ||
                String.Equals(queued, "True", StringComparison.OrdinalIgnoreCase) ||
                !String.Equals(delivered, "True", StringComparison.OrdinalIgnoreCase) ||
                String.IsNullOrWhiteSpace(turnId) ||
                !String.IsNullOrWhiteSpace(error) ||
                !String.IsNullOrWhiteSpace(messageError))
            {
                throw new Exception("Strict send failed: delivery=" + delivery + " delivered=" + delivered + " queued=" + queued + " turnId=" + turnId + " error=" + FirstNonEmpty(error, messageError));
            }
        }

        private static string FirstNonEmpty(string a, string b)
        {
            if (!String.IsNullOrWhiteSpace(a)) return a;
            if (!String.IsNullOrWhiteSpace(b)) return b;
            return "";
        }

        private static string NestedValue(Dictionary<string, object> map, params string[] keys)
        {
            object current = map;
            foreach (string key in keys)
            {
                Dictionary<string, object> currentMap = current as Dictionary<string, object>;
                if (currentMap == null || !currentMap.ContainsKey(key) || currentMap[key] == null) return "";
                current = currentMap[key];
            }
            return Convert.ToString(current);
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

        private List<Dictionary<string, object>> LoadPeers()
        {
            Dictionary<string, object> result = ApiGet("/peers");
            if (result.ContainsKey("peers") && result["peers"] is ArrayList)
            {
                return ConvertAgentArray((ArrayList)result["peers"]);
            }
            return new List<Dictionary<string, object>>();
        }

        private bool IsAgentTestable(Dictionary<string, object> agent)
        {
            string threadId = Value(agent, "threadId");
            string status = Value(agent, "status");
            string threadSource = Value(agent, "threadSource");
            string route = Value(agent, "route");
            if (String.IsNullOrWhiteSpace(threadId)) return false;
            if (String.Equals(status, "stale", StringComparison.OrdinalIgnoreCase)) return false;
            if (String.Equals(status, "unbound", StringComparison.OrdinalIgnoreCase)) return false;
            if (String.Equals(threadSource, "mailbox", StringComparison.OrdinalIgnoreCase)) return false;
            if (String.Equals(threadSource, "antigravity", StringComparison.OrdinalIgnoreCase)) return false;
            if (String.IsNullOrWhiteSpace(route)) return false;
            return true;
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
                        agent["sourceHost"] = Value(thread, "sourceHost");
                        agent["hostKind"] = Value(thread, "hostKind");
                        agent["transport"] = Value(thread, "transport");
                        agent["route"] = Value(thread, "route");
                        if (!String.IsNullOrWhiteSpace(Value(thread, "nodeName")))
                        {
                            agent["node"] = Value(thread, "nodeName");
                        }
                    }
                    filtered.Add(agent);
                }
            }
            log("active-filter applied active=" + filtered.Count + " total=" + agents.Count + " skipped=" + (agents.Count - filtered.Count));
            return filtered;
        }

        private HashSet<string> LoadActiveThreadIds()
        {
            HashSet<string> ids = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            activeThreadMetadata = new Dictionary<string, Dictionary<string, object>>(StringComparer.OrdinalIgnoreCase);
            try
            {
                Dictionary<string, object> response = ApiGet("/agents");
                if (response != null && response.ContainsKey("agents") && response["agents"] is ArrayList)
                {
                    foreach (object row in (ArrayList)response["agents"])
                    {
                        Dictionary<string, object> agent = row as Dictionary<string, object>;
                        if (agent == null) continue;
                        string threadId = Value(agent, "threadId");
                        if (String.IsNullOrWhiteSpace(threadId)) continue;
                        ids.Add(threadId);
                        activeThreadMetadata[threadId] = agent;
                    }
                    log("active-classifier-loaded count=" + ids.Count + " source=daemon-registry");
                }
            }
            catch (Exception ex)
            {
                log("active-classifier-error source=daemon-registry " + ex.Message);
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

        private static bool ValueBool(Dictionary<string, object> map, string key)
        {
            if (map == null || !map.ContainsKey(key) || map[key] == null) return false;
            object value = map[key];
            if (value is bool) return (bool)value;
            string text = Convert.ToString(value);
            return String.Equals(text, "true", StringComparison.OrdinalIgnoreCase) || text == "1";
        }

        private static int ToInt(string value)
        {
            int parsed;
            if (Int32.TryParse(value, out parsed)) return parsed;
            return 0;
        }

        private static string DisplayTitle(Dictionary<string, object> agent)
        {
            string title = Value(agent, "chatTitle");
            if (!String.IsNullOrWhiteSpace(title)) return title;
            string name = Value(agent, "name");
            if (!String.IsNullOrWhiteSpace(name)) return name;
            return Value(agent, "threadId");
        }

        private static string JoinList(Dictionary<string, object> map, string key)
        {
            if (map == null || !map.ContainsKey(key) || map[key] == null) return "";
            ArrayList list = map[key] as ArrayList;
            if (list == null) return Convert.ToString(map[key]);
            List<string> values = new List<string>();
            foreach (object item in list)
            {
                if (item == null) continue;
                values.Add(Convert.ToString(item));
            }
            return String.Join(", ", values.ToArray());
        }

        private static string ShortIso(string value)
        {
            if (String.IsNullOrWhiteSpace(value)) return "";
            DateTime parsed;
            if (!DateTime.TryParse(value, out parsed)) return value;
            return parsed.ToLocalTime().ToString("M/d HH:mm:ss");
        }

        private bool TryStartDaemon()
        {
            lock (daemonStartLock)
            {
                if (daemonStartAttempted || daemonStartInProgress)
                {
                    log("daemon-start-skipped reason=already-attempted-or-in-progress");
                    return false;
                }
                daemonStartAttempted = true;
                daemonStartInProgress = true;
            }
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
            finally
            {
                lock (daemonStartLock)
                {
                    daemonStartInProgress = false;
                }
            }
        }

        private bool RecoverFromForeignDaemon()
        {
            lock (daemonStartLock)
            {
                if (daemonRecoveryAttempted || daemonStartInProgress)
                {
                    log("daemon-recovery-skipped reason=already-attempted");
                    return false;
                }
                daemonRecoveryAttempted = true;
                daemonStartAttempted = false;
            }

            try
            {
                ShutdownExistingDaemon();
                Thread.Sleep(1200);
                return TryStartDaemon();
            }
            catch (Exception ex)
            {
                log("daemon-recovery-error " + ex.Message);
                return false;
            }
        }

        private void ShutdownExistingDaemon()
        {
            int port = CamPaths.Port;
            int currentPid = Process.GetCurrentProcess().Id;
            try
            {
                HttpWebRequest request = (HttpWebRequest)WebRequest.Create("http://127.0.0.1:" + port + "/shutdown");
                request.Method = "POST";
                request.Timeout = 2000;
                using (HttpWebResponse response = (HttpWebResponse)request.GetResponse())
                {
                }
                log("daemon-recovery-shutdown-sent port=" + port);
            }
            catch (Exception ex)
            {
                log("daemon-recovery-shutdown-ignored port=" + port + " error=" + ex.Message);
            }

            if (!WaitForPortRelease(port, 3000))
            {
                KillPortOccupant(port, currentPid);
                WaitForPortRelease(port, 3000);
            }
        }

        private bool WaitForPortRelease(int port, int timeoutMs)
        {
            Stopwatch stopwatch = Stopwatch.StartNew();
            while (stopwatch.ElapsedMilliseconds < timeoutMs)
            {
                if (!IsPortInUse(port))
                {
                    log("daemon-recovery-port-free port=" + port);
                    return true;
                }
                Thread.Sleep(150);
            }
            log("daemon-recovery-port-still-busy port=" + port);
            return false;
        }

        private bool IsPortInUse(int port)
        {
            TcpClient client = new TcpClient();
            try
            {
                IAsyncResult result = client.BeginConnect("127.0.0.1", port, null, null);
                bool ok = result.AsyncWaitHandle.WaitOne(250);
                if (!ok) return false;
                client.EndConnect(result);
                return true;
            }
            catch
            {
                return false;
            }
            finally
            {
                try { client.Close(); } catch { }
            }
        }

        private void KillPortOccupant(int port, int currentPid)
        {
            try
            {
                int? pid = FindOwningPidByNetstat(port, currentPid);
                if (!pid.HasValue)
                {
                    log("daemon-recovery-port-owner-missing port=" + port);
                    return;
                }
                Process process = Process.GetProcessById(pid.Value);
                log("daemon-recovery-kill pid=" + pid.Value + " name=" + process.ProcessName);
                process.Kill();
                process.WaitForExit(4000);
            }
            catch (Exception ex)
            {
                log("daemon-recovery-kill-failed port=" + port + " error=" + ex.Message);
            }
        }

        private int? FindOwningPidByNetstat(int port, int currentPid)
        {
            try
            {
                ProcessStartInfo psi = new ProcessStartInfo();
                psi.FileName = "netstat.exe";
                psi.Arguments = "-ano -p tcp";
                psi.UseShellExecute = false;
                psi.RedirectStandardOutput = true;
                psi.RedirectStandardError = true;
                psi.CreateNoWindow = true;
                psi.WindowStyle = ProcessWindowStyle.Hidden;
                using (Process process = Process.Start(psi))
                {
                    string output = process.StandardOutput.ReadToEnd();
                    process.WaitForExit(3000);
                    string[] lines = output.Split(new[] { "\r\n", "\n" }, StringSplitOptions.RemoveEmptyEntries);
                    string token = "127.0.0.1:" + port;
                    foreach (string line in lines)
                    {
                        string trimmed = line.Trim();
                        if (!trimmed.StartsWith("TCP", StringComparison.OrdinalIgnoreCase)) continue;
                        string[] parts = trimmed.Split((char[])null, StringSplitOptions.RemoveEmptyEntries);
                        if (parts.Length < 5) continue;
                        string localAddress = parts[1];
                        string state = parts[3];
                        string pidText = parts[4];
                        if (!String.Equals(localAddress, token, StringComparison.OrdinalIgnoreCase)) continue;
                        if (!String.Equals(state, "LISTENING", StringComparison.OrdinalIgnoreCase)) continue;
                        int pid;
                        if (!Int32.TryParse(pidText, out pid)) continue;
                        if (pid == currentPid) continue;
                        return pid;
                    }
                }
            }
            catch (Exception ex)
            {
                log("daemon-recovery-netstat-failed port=" + port + " error=" + ex.Message);
            }
            return null;
        }

        private bool IsUnauthorized(Exception ex)
        {
            WebException web = ex as WebException;
            if (web == null) return ex.Message.IndexOf("401", StringComparison.OrdinalIgnoreCase) >= 0 ||
                ex.Message.IndexOf("Unauthorized", StringComparison.OrdinalIgnoreCase) >= 0;
            HttpWebResponse response = web.Response as HttpWebResponse;
            if (response != null && response.StatusCode == HttpStatusCode.Unauthorized) return true;
            return ex.Message.IndexOf("401", StringComparison.OrdinalIgnoreCase) >= 0 ||
                ex.Message.IndexOf("Unauthorized", StringComparison.OrdinalIgnoreCase) >= 0;
        }

        private static string AppendLine(string existing, string line)
        {
            if (String.IsNullOrEmpty(existing)) return DateTime.Now.ToString("T") + "  " + line;
            return existing + Environment.NewLine + DateTime.Now.ToString("T") + "  " + line;
        }

        private void AppendOutput(string line)
        {
            outputBox.Text = AppendLine(outputBox.Text, line);
            outputBox.SelectionStart = outputBox.Text.Length;
            outputBox.SelectionLength = 0;
            outputBox.ScrollToCaret();
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

        public static string Version
        {
            get { return "2.1.49"; }
        }
    }
}
