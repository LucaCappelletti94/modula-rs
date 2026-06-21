using SampleCsharp.Mathx;

namespace SampleCsharp
{
    public class Program
    {
        public static int Greet(int n)
        {
            return Mathx.Mathx.Add(1, n);
        }

        public static void Main(string[] args)
        {
            Greet(2);
        }
    }
}
