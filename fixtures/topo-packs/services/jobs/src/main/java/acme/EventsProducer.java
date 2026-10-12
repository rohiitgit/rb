package acme;

import org.springframework.kafka.core.KafkaTemplate;

public class EventsProducer {
    private final KafkaTemplate<String, String> template;

    public EventsProducer(KafkaTemplate<String, String> template) {
        this.template = template;
    }

    public void publish(String event) {
        template.send("${app.topic}", event);
        template.send("events-" + event, event);
    }
}
