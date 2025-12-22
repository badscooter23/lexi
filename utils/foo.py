
import pprint
import os
import json

from openai import OpenAI
client = OpenAI()

question="Using what you know about woodchucks and the rate at whcih they chuck wood, How much wood would a woodchuck chuck if a woodchuck was given 20 minutes to chuck wood? Give the answer in pounds and explain your answer."
print(f"\n{question}")

def line(ch="-", n=50):
    print()
    print(ch*n)

def section(model):
    print()
    line("=")
    print(f"Using Model {model}")
    line("=")

section("gpt-5.1")
response = client.responses.create(
    model="gpt-5.1",
    input=question
)
print("Input tokens:", response.usage.input_tokens)
print("Output tokens:", response.usage.output_tokens)
print("Reasoning tokens:", response.usage.output_tokens_details.reasoning_tokens)

line()
print(response.output_text)
line()

section("gpt-5.2")
response = client.responses.create(
    model="gpt-5.2",
    input=question
)
print("Input tokens:", response.usage.input_tokens)
print("Output tokens:", response.usage.output_tokens)
print("Reasoning tokens:", response.usage.output_tokens_details.reasoning_tokens)

line()
print(response.output_text)
line()


section("nvidia: meta/llama-3.1-70b-instruct")
nv_client = OpenAI(
      api_key=os.environ["NVIDIA_API_KEY"],
      base_url="https://integrate.api.nvidia.com/v1",
)

response = nv_client.chat.completions.create(
      model="meta/llama-3.1-70b-instruct",
      messages=[{"role": "user", "content": question}],
)
line()
print(response.choices[0].message.content)
line()
