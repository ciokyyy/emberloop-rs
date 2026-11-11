Run the shell command in `command`, in the Project root. Commands are token-compressed via rtk; prefix rtk bypass to one command that needs its unfiltered output. `timeout` is in seconds, defaults to 120 and caps at 600. With `background` set to true the command starts detached and answers with a handle and its first output.

Refused before they run, each naming the tool to use: `sed -i`, `perl -i` and `awk -i inplace` (`edit`); inline Python such as `python3 -c` or a heredoc (`execute`); a file redirect or tee (`write`); a trailing `&` or nohup (`background`).
