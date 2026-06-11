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
            string output = RunCamCommand("daemon status");
            MessageBox.Show(output, "CAM Daemon Status", MessageBoxButtons.OK, MessageBoxIcon.Information);
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
