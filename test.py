"""Just some tests."""

from time import sleep

from craft_cli.printer import Mode, Printer

printer = Printer(Mode.Verbose)

printer.progress("hey!", permanent=True)
printer.progress("starting", permanent=True)
sleep(10)
print("hello?")
printer.progress("processing", permanent=False)
sleep(2)
printer.progress("end", permanent=False)
