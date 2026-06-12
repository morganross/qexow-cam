using System;
using System.Reflection;
using System.Windows.Forms;

class Test
{
    [STAThread]
    static void Main()
    {
        try {
            var assembly = Assembly.LoadFrom(@"C:\dev\AG Bridge end Cam\qexow-cam\dist\cam.exe");
            var type = assembly.GetType("CamTray.TrayApplicationContext");
            var ctx = Activator.CreateInstance(type);
            
            Console.WriteLine("Invoking ShowStatusDialog...");
            var method = type.GetMethod("ShowStatusDialog", BindingFlags.NonPublic | BindingFlags.Instance);
            
            var timer = new Timer { Interval = 2000 };
            timer.Tick += (s, e) => {
                Console.WriteLine("Timer tick, closing...");
                Application.Exit();
            };
            timer.Start();

            method.Invoke(ctx, null);
            Console.WriteLine("Done without exceptions.");
        } catch (Exception ex) {
            Console.WriteLine("Exception: " + ex.ToString());
        }
    }
}
