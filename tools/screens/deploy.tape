Output "/out/deploy.gif"
Set Shell "bash"
Set FontSize 18
Set Width 1280
Set Height 660
Set Padding 28
Set Margin 24
Set MarginFill "#F1EAEC"
Set BorderRadius 10
Set WindowBar Colorful
Set TypingSpeed 45ms
Set Theme {"name":"turnout","black":"#150E11","red":"#F87171","green":"#6FBF8A","yellow":"#E8A33D","blue":"#A98F97","magenta":"#F0788C","cyan":"#F0788C","white":"#DFD3D6","brightBlack":"#837076","brightRed":"#F87171","brightGreen":"#6FBF8A","brightYellow":"#E8A33D","brightBlue":"#A98F97","brightMagenta":"#F0919F","brightCyan":"#F0919F","brightWhite":"#EFE2E6","background":"#150E11","foreground":"#DFD3D6","selection":"#322028","cursor":"#F0788C"}
Hide
Type "source /work/setup.sh" Enter
Sleep 4s
Show
Sleep 600ms
Type "turnout deploy web-staging --no-build --backup"
Sleep 700ms
Enter
Sleep 15s
Screenshot "/out/deploy.png"
Sleep 3s
