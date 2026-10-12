using Microsoft.Extensions.DependencyInjection;

namespace Shop;

public class Startup
{
    public void ConfigureServices(IServiceCollection services)
    {
        services.AddSingleton<IMailer, Mailer>();
        services.AddTransient<Helper>();
        services.AddTransient<MailerUser>();
    }
}
