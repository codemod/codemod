// Console.WriteLine() will be matched by pattern!
// click diff tab to see rewrite.

using System;

namespace MyApp
{
    public class Program
    {
        public static void Main(string[] args)
        {
            Console.WriteLine("Hello, World!");
            Console.Write("Debug message: ");
            Console.WriteLine($"User ID: {userId}");
            
            if (isDebugMode)
            {
                Console.WriteLine("Application started successfully");
                Console.Write("Processing...");
            }
            
            // This won't be matched
            var message = "Console.WriteLine(\"not me\")";
            SomeOtherClass.WriteLine("also not matched");
        }
        
        private static void ProcessData()
        {
            Console.WriteLine("Processing data");
            Console.Write("Progress: 50%");
        }
    }
}