using Microsoft.Extensions.DependencyInjection;

namespace Shop;

public interface IKeyed
{
    string Read(string name);
}

public class KeyedA : IKeyed
{
    public string Read(string name) => "a";
}

public class KeyedB : IKeyed
{
    public string Read(string name) => "b";
}

public class KeyedStartup
{
    public void ConfigureKeyed(IServiceCollection services)
    {
        services.AddKeyedSingleton<IKeyed, KeyedA>("main");
        services.AddKeyedSingleton<IKeyed, KeyedB>("other");
        services.AddTransient<KeyedConsumer>();
    }
}

public class KeyedConsumer
{
    [Inject]
    public Helper Aux { get; set; }

    // Unmarked properties stay silent (contract M6k S5): no point, no edge.
    public Helper Plain { get; set; }

    public KeyedConsumer([FromKeyedServices("main")] IKeyed keyed)
    {
    }
}
