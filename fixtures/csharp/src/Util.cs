namespace Shop {
    public static class Util {
        public static string Shout(string s) {
            return s;
        }
    }

    public interface IGreeter {
        string Greet(string n);
    }

    public class Greeter : IGreeter {
        public string Greet(string n) {
            return Util.Shout(n);
        }
    }
}
