# Writes echo_pairs.tsv: python3 echo_pairs.py echo_pairs.tsv. Seeded, so the file is the same each time.
import random, sys
random.seed(13)
call = """we should ship the release on friday if the tests pass
can everyone see my screen right now
the budget numbers came in lower than we expected this quarter
let's move the design review to next tuesday afternoon
i talked to the customer and they want the export feature first
does anyone have questions about the migration plan
the server went down twice last night because of the disk
sarah will send the notes after this meeting ends
we need two more people on the mobile team by march
i think the new onboarding flow is much easier to follow
please update the ticket when you finish the first draft
the contract has to be signed before the end of the month
yes
okay sounds good
thanks everyone
can you hear me
i'm sorry i was on mute
the latency is about two hundred milliseconds on the new model
our users in germany asked for a translated help page
we can talk about pricing at the next call
could you share the link to that document in the chat
the marketing team wants a short video for the launch
i don't think that estimate includes the testing work
the first quarter was strong but the second one looks weak
let me pull up the dashboard and walk through the graph
we lost about ten percent of the traffic after the redesign
he said the vendor can deliver the hardware in six weeks
that's a good point and we should write it down
the security audit found three small issues and one large one
let's take a five minute break and come back at ten""".split("\n")
replies = """yes i can take that one
no i haven't looked at it yet
i think we should wait for the test results first
my internet was slow this morning so i missed that part
i'll send you the file when i get back to my desk
that works for me on tuesday
can we talk about the budget after lunch
i agree with what you said about the customer
what time is the meeting tomorrow
i don't have the numbers in front of me right now
let me check with my manager and get back to you
we had the same problem with the server last year
sure
no thanks
right
i think so
i'm not sure about that
the release can wait until we know more
hold on my dog is barking
we should talk to sarah about the notes
can you repeat the last part of that please
i was going to say the same thing
it depends on how much testing we need to do
i wrote the first draft yesterday and it needs work
give me one second to find the link
that's what i said in the last call too
i'm going to mute myself for a moment
we can do that but not this month
the team in germany told me something else
ten percent sounds high to me""".split("\n")
similar = {"the":"a","to":"too","two":"to","there":"their","we":"he","our":"are","i":"a","you":"your","can":"can't","on":"in","and":"in","for":"four","see":"sea","right":"write","it":"its","is":"as","this":"that","will":"we'll","were":"where","now":"no","by":"buy","meeting":"meetings","release":"releases","numbers":"number","plan":"plans","tests":"test"}
wrong = "well like so uh um then just really maybe kind of sort very good thing way yeah".split()
def noise(ws, rate):
    out=[]
    for w in ws:
        r=random.random()
        if r < rate/2: continue                     # A word that the model did not hear.
        if r < rate*0.8: out.append(similar.get(w, random.choice(wrong)))  # A word that it heard wrong.
        elif r < rate: out.extend([w, random.choice(wrong)])               # A word that it added.
        else: out.append(w)
    return out or ws[:1]
rows=["# Test data of the echo rule (docs/sdk.md, \"System audio\"). Synthetic: echo_pairs.py writes it.","# echo: the microphone text is the system text after the errors of a second transcript (10 to 30 % of the words wrong, missing, or added), sometimes with the first or last words cut.","# other: the microphone text is what the user says, and the system text is what the call says at that time. They share common words.","# kind\tmicrophone\tsystem"]
for s in call:
    ws=s.split()
    for rate in (0.1,0.2,0.3):
        for _ in range(3):
            m=noise(ws,rate)
            if len(m)>=6 and random.random()<0.3:
                m = m[random.randint(1,2):] if random.random()<0.5 else m[:-random.randint(1,2)]
            rows.append("echo\t%s\t%s"%(" ".join(m),s))
for r in replies:
    for s in call:
        rows.append("other\t%s\t%s"%(r,s))
open(sys.argv[1],"w").write("\n".join(rows)+"\n")
