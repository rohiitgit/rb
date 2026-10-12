package acme;

import org.springframework.kafka.annotation.KafkaListener;

public class EventsConsumer {
    @KafkaListener(topics = "prod-events")
    public void handle(String event) {
    }
}
