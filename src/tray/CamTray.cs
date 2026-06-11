using System;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Windows.Forms;

namespace CamTray
{
    static class Program
    {
        [STAThread]
        static void Main()
        {
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            Application.Run(new TrayApplicationContext());
        }
    }

    public class TrayApplicationContext : ApplicationContext
    {
        private NotifyIcon trayIcon;

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
                Text = "Codex Agent Manager"
            };

            trayIcon.DoubleClick += Status_Click;
        }

        private void Status_Click(object sender, EventArgs e)
        {
            string output = RunCamCommand("doctor");

            Form statusForm = new Form();
            statusForm.Text = "CAM System Status";
            statusForm.Size = new System.Drawing.Size(760, 520);
            statusForm.MinimumSize = new System.Drawing.Size(500, 300);
            statusForm.StartPosition = FormStartPosition.CenterScreen;
            statusForm.BackColor = System.Drawing.Color.FromArgb(15, 15, 25);
            statusForm.ForeColor = System.Drawing.Color.White;

            RichTextBox rtb = new RichTextBox();
            rtb.Dock = DockStyle.Fill;
            rtb.ReadOnly = true;
            rtb.Font = new System.Drawing.Font("Consolas", 10f);
            rtb.BackColor = System.Drawing.Color.FromArgb(15, 15, 25);
            rtb.ForeColor = System.Drawing.Color.White;
            rtb.BorderStyle = BorderStyle.None;
            rtb.ScrollBars = RichTextBoxScrollBars.Vertical;

            string raw = string.IsNullOrWhiteSpace(output) ? "No output from cam doctor." : output;
            string[] outputLines = raw.Replace("\r\n", "\n").Replace("\r", "\n").Split('\n');
            foreach (string outputLine in outputLines)
            {
                int start = rtb.TextLength;
                rtb.AppendText(outputLine + "\n");
                rtb.Select(start, outputLine.Length);
                if (outputLine.StartsWith("OK "))
                    rtb.SelectionColor = System.Drawing.Color.LimeGreen;
                else if (outputLine.StartsWith("BAD"))
                    rtb.SelectionColor = System.Drawing.Color.OrangeRed;
                else if (outputLine.StartsWith("[") && outputLine.EndsWith("]"))
                    rtb.SelectionColor = System.Drawing.Color.CornflowerBlue;
                else
                    rtb.SelectionColor = System.Drawing.Color.Silver;
            }
            rtb.SelectionStart = 0;
            rtb.SelectionLength = 0;

            statusForm.Controls.Add(rtb);
            statusForm.ShowDialog();
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
                string exeDir = AppDomain.CurrentDomain.BaseDirectory;
                string camExe = Path.Combine(exeDir, "cam.exe");

                if (!File.Exists(camExe))
                {
                    camExe = "cam.exe"; // Fallback to PATH
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
