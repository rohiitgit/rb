namespace Shop;

public interface IMailer
{
    string Greet(string name);
}

public class Mailer : IMailer
{
    public string Greet(string name) => "hi " + name;
}
