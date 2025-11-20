// Console.WriteLine() will be matched by pattern!
// click diff tab to see rewrite.

using System;

namespace MyApp
{
    public class Program
    {
        public static void Main(string[] args)
        {
            Logger.Log("Hello, World!");
            Logger.Log("Debug message: ");
            Logger.Log($"User ID: {userId}");
            
            if (isDebugMode)
            {
                Logger.Log("Application started successfully");
                Logger.Log("Processing...");
            }
            
            // This won't be matched
            var message = "Console.WriteLine(\"not me\")";
            SomeOtherClass.WriteLine("also not matched");
        }
        
        private static void ProcessData()
        {
            Logger.Log("Processing data");
            Logger.Log("Progress: 50%");
        }
    }
}
