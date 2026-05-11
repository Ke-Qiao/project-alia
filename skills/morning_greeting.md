+++
name = "morning_greeting"
display_name = "早安问候"
description = "根据时间和天气生成个性化早安问候"
category = "utility"
intent_patterns = ["早安", "早上好", "good morning", "起床"]
tools = ["get_current_time", "sensor_read"]
priority = 3
cacheable = false
enabled = true

[parameters]
type = "object"

[parameters.properties.user_name]
type = "string"
description = "用户称呼"
+++

用户说了早安。请根据当前时间和用户习惯，生成一段温暖的早安问候。
如果知道用户的日程安排，可以简要提醒今天的重要事项。
