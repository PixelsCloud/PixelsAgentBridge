on run argv
    activate
    display alert "Pixels Agent Bridge 安装未完成 / Installation incomplete" message (item 1 of argv) as critical buttons {"好 / OK"} default button 1 giving up after 90
end run
