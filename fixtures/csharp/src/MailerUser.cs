namespace Shop;

public class MailerUser
{
    private readonly IMailer greeter;

    public MailerUser(IMailer greeter, Helper helper)
    {
        this.greeter = greeter;
    }
}
