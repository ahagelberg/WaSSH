import type { MqttAnalyserSavedMessage } from './protocol'

/** Default MQTT broker host on the remote machine */
export const MQTT_ANALYSER_DEFAULT_HOST = '127.0.0.1'

/** Default plain MQTT port */
export const MQTT_ANALYSER_DEFAULT_PORT = 1883

/** Max retained history entries per topic in the UI */
export const MQTT_ANALYSER_HISTORY_LIMIT = 100

/** Brief highlight duration when a topic receives a message (ms) */
export const MQTT_ANALYSER_BLINK_MS = 500

/** Default saved publish messages (empty — users define their own) */
export const MQTT_ANALYSER_DEFAULT_MESSAGES: MqttAnalyserSavedMessage[] = []

/** Whether the saved-messages section starts collapsed */
export const MQTT_ANALYSER_MESSAGES_COLLAPSED_DEFAULT = true

/** Base name for newly created saved messages */
export const MQTT_ANALYSER_NEW_MESSAGE_LABEL = 'New message'
