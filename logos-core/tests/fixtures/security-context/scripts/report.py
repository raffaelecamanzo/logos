import subprocess


def render(name):
    output = subprocess.check_output("report " + name, shell=True)
    return output.decode()
